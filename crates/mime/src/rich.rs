// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Rich text (`mailcraft_model::rich`) to HTML and back.
//!
//! [`to_html`] writes the owner's text as the HTML part of a message: `<div>` paragraphs, `<ul>`
//! and `<ol>` lists, `<b>`, `<i>`, `<u>`, `<span style="…">` with a font size, a colour and a
//! font from the model's list, `<a href>`, and pictures as `<img src="cid:…">` when the text's
//! holder says which ([`to_html_with`]), and nothing else. Every character of text and of an
//! address is escaped, and a link is written only when its address is one the model accepts
//! (`rich::safe_link`: any scheme but `javascript`, `vbscript`, `data`, `file` and `cid`). [`signature_html`] lays a signature out, its first picture
//! beside or above the text when it asks for that, in a two-cell table, as Outlook writes it.
//!
//! [`from_html`] is the way back, and the way in for anything pasted: the HTML is read by
//! `mailcraft_html` (which never runs or fetches anything) and what it gives is reduced to the
//! set (a link keeps only an address a reader follows, `http`, `https`, `mailto`, `tel` or `cid`; a
//! draft of this program's, [`from_html_with_pictures`], keeps any the model accepts). Tables give their cells' paragraphs in reading order; colours are kept; pictures, rules,
//! fonts, alignment and everything else are dropped.

use mailcraft_html::{Block, BlockKind, Marker, Span};
use mailcraft_model::rich::{Font, ListKind, MAX_LEVEL, MAX_SIZE, MIN_SIZE, OBJECT, Paragraph, RichText, Run, Style, safe_color, safe_link};
use mailcraft_model::signature::{Layout, Picture, Signature};

/// How a picture of the text is written: its `cid:` address (without the scheme), the width it
/// is shown at and its description.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Img {
    pub content_id: String,
    pub width: Option<u16>,
    pub alt: String,
}

impl Img {
    /// A signature's picture, sent as the inline part `content_id`.
    pub fn of(picture: &Picture, content_id: impl Into<String>) -> Img {
        Img { content_id: content_id.into(), width: picture.width, alt: picture.alt.clone() }
    }
}

/// The text as an HTML fragment (what goes inside `<body>`). A list item a level deeper than the
/// one before it is in a list of its own inside that item (`<ul><li>a<ul><li>b</li></ul></li></ul>`),
/// as Outlook, Word and the web's editors write nested lists.
pub fn to_html(text: &RichText) -> String {
    to_html_with(text, &|_| None)
}

/// [`to_html`], with each picture written as `pictures` says (left out when it says nothing).
pub fn to_html_with(text: &RichText, pictures: &dyn Fn(u32) -> Option<Img>) -> String {
    let mut out = String::new();
    // The lists open, outermost first: each with its kind and whether an item of it is open.
    let mut open: Vec<(ListKind, bool)> = Vec::new();
    let paragraphs = text.paragraphs();
    for (i, p) in paragraphs.iter().enumerate() {
        if p.list == ListKind::None {
            close_to(&mut out, &mut open, 0);
            out.push_str("<div>");
            paragraph(&mut out, p, pictures);
            out.push_str("</div>\r\n");
            continue;
        }
        let depth = usize::from(p.level.min(MAX_LEVEL)) + 1;
        close_to(&mut out, &mut open, depth);
        // A list of the other kind at this level ends the one that is open there.
        if open.len() == depth && open.last().is_some_and(|(kind, _)| *kind != p.list) {
            close_to(&mut out, &mut open, depth - 1);
        }
        // The item before at this level ends here.
        if open.len() == depth
            && let Some((_, item)) = open.last_mut()
            && *item
        {
            out.push_str("</li>");
            *item = false;
        }
        while open.len() < depth {
            out.push_str(if p.list == ListKind::Numbered {
                "<ol style=\"margin-top:0;margin-bottom:0\">"
            } else {
                "<ul style=\"margin-top:0;margin-bottom:0\">"
            });
            open.push((p.list, false));
        }
        out.push_str("<li>");
        if let Some((_, item)) = open.last_mut() {
            *item = true;
        }
        paragraph(&mut out, p, pictures);
        // The item stays open for a deeper list that follows it.
        let deeper = paragraphs.get(i + 1).is_some_and(|next| next.list != ListKind::None && next.level > p.level);
        if !deeper {
            out.push_str("</li>");
            if let Some((_, item)) = open.last_mut() {
                *item = false;
            }
        }
        out.push_str("\r\n");
    }
    close_to(&mut out, &mut open, 0);
    out
}

/// Close lists (and their open items) until `depth` are left open.
fn close_to(out: &mut String, open: &mut Vec<(ListKind, bool)>, depth: usize) {
    while open.len() > depth {
        let Some((kind, item)) = open.pop() else { break };
        if item {
            out.push_str("</li>");
        }
        out.push_str(if kind == ListKind::Numbered { "</ol>" } else { "</ul>" });
    }
}

/// A paragraph's runs, or a line break for an empty one.
fn paragraph(out: &mut String, p: &Paragraph, pictures: &dyn Fn(u32) -> Option<Img>) {
    if p.is_empty() {
        out.push_str("<br>");
    } else {
        runs(out, p, pictures);
    }
}

/// A picture as `<img>`: its `cid:` address, width and description, all escaped.
fn img(out: &mut String, img: &Img) {
    let cid: String = img.content_id.chars().filter(|c| c.is_ascii_graphic() && !matches!(c, '<' | '>' | '"')).collect();
    out.push_str("<img src=\"cid:");
    out.push_str(&mailcraft_html::escape(&cid));
    out.push('"');
    if let Some(w) = img.width {
        out.push_str(&format!(" width=\"{}\"", w.clamp(1, mailcraft_model::pictures::MAX_SHOWN_WIDTH)));
    }
    out.push_str(" alt=\"");
    out.push_str(&mailcraft_html::escape(&img.alt.chars().filter(|c| !c.is_control()).collect::<String>()));
    out.push_str("\" style=\"border:0\">");
}

/// A signature as HTML, its pictures written as `content_id` names them. Laid out with its first
/// picture on the left of the text or above it when it asks for that: a two-cell table, or the
/// picture in a paragraph of its own.
pub fn signature_html(sig: &Signature, content_id: &dyn Fn(&Picture) -> String) -> String {
    let pictures = |id: u32| sig.picture(id).map(|p| Img::of(p, content_id(p)));
    let first = sig.body().pictures().first().copied().filter(|_| sig.layout != Layout::Inline);
    let Some((first, lead)) = first.and_then(|id| pictures(id).map(|i| (id, i))) else {
        return to_html_with(sig.body(), &pictures);
    };
    // The text without that picture.
    let rest = RichText::from_paragraphs(
        sig.body()
            .paragraphs()
            .iter()
            .map(|p| Paragraph::new(p.runs.iter().filter(|r| r.picture != Some(first)).cloned().collect(), p.list).at_level(p.level))
            .collect(),
    );
    let text = to_html_with(&rest, &pictures);
    let mut out = String::new();
    if sig.layout == Layout::PictureLeft {
        out.push_str("<table cellpadding=\"0\" cellspacing=\"0\" border=\"0\"><tr><td style=\"vertical-align:top;padding-right:12px\">");
        img(&mut out, &lead);
        out.push_str("</td><td style=\"vertical-align:top\">\r\n");
        out.push_str(&text);
        out.push_str("</td></tr></table>\r\n");
    } else {
        out.push_str("<div>");
        img(&mut out, &lead);
        out.push_str("</div>\r\n");
        out.push_str(&text);
    }
    out
}

/// A whole HTML document holding the text, and under it `below` as it is (a quoted original).
pub fn to_html_document(text: &RichText, below: Option<&str>) -> String {
    document(&to_html(text), below)
}

/// A whole HTML document holding `fragment` (HTML this module wrote), and under it `below` as it
/// is.
pub fn document(fragment: &str, below: Option<&str>) -> String {
    let mut out = String::from("<html><head><meta http-equiv=\"Content-Type\" content=\"text/html; charset=utf-8\"></head><body>\r\n");
    out.push_str(fragment);
    if let Some(below) = below {
        out.push_str(below);
        out.push_str("\r\n");
    }
    out.push_str("</body></html>\r\n");
    out
}

/// The runs of a paragraph. White space is kept as typed: a space at the start or the end of the
/// paragraph, or after another, is a no-break space, and a tab is four.
fn runs(out: &mut String, p: &Paragraph, pictures: &dyn Fn(u32) -> Option<Img>) {
    let total: usize = p.runs.iter().map(|r| r.text.chars().count()).sum();
    let mut index = 0usize;
    let mut after_space = true;
    for run in &p.runs {
        let style = run.style.clone().clean();
        if let Some(id) = run.picture {
            index += 1;
            after_space = false;
            if let Some(found) = pictures(id) {
                img(out, &found);
            }
            continue;
        }
        let mut close: Vec<&str> = Vec::new();
        if let Some(url) = &style.link {
            out.push_str("<a href=\"");
            out.push_str(&mailcraft_html::escape(url));
            out.push_str("\">");
            close.push("</a>");
        }
        for (on, open, end) in [(style.bold, "<b>", "</b>"), (style.italic, "<i>", "</i>"), (style.underline, "<u>", "</u>")] {
            if on {
                out.push_str(open);
                close.push(end);
            }
        }
        if let Some(size) = style.size {
            out.push_str(&format!("<span style=\"font-size:{}pt\">", size.clamp(MIN_SIZE, MAX_SIZE)));
            close.push("</span>");
        }
        if let Some(color) = style.color.as_deref().and_then(safe_color) {
            out.push_str(&format!("<span style=\"color:{color}\">"));
            close.push("</span>");
        }
        if let Some(font) = style.font {
            out.push_str(&format!("<span style=\"font-family:{}\">", font.css()));
            close.push("</span>");
        }
        for c in run.text.chars() {
            index += 1;
            match c {
                ' ' if after_space || index == total => out.push_str("&nbsp;"),
                ' ' => out.push(' '),
                '\t' => out.push_str("&nbsp;&nbsp;&nbsp;&nbsp;"),
                '&' => out.push_str("&amp;"),
                '<' => out.push_str("&lt;"),
                '>' => out.push_str("&gt;"),
                '"' => out.push_str("&quot;"),
                '\'' => out.push_str("&#39;"),
                c if c.is_control() => {}
                c => out.push(c),
            }
            after_space = c == ' ' || c == '\t';
        }
        for end in close.iter().rev() {
            out.push_str(end);
        }
    }
}

/// Any HTML (a message, or what was pasted) reduced to rich text.
///
/// Blank lines at the very start and end of the text are kept when they are written the way
/// [`to_html`] writes them (`<div><br></div>`): a reader shows nothing there, but the owner put
/// them there.
pub fn from_html(html: &str) -> RichText {
    read(html, false)
}

/// An `<img src="cid:…">` of an HTML draft this program wrote.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImgRef {
    /// The Content-ID, without `cid:`.
    pub cid: String,
    /// The `width` it was written with.
    pub width: Option<u16>,
    /// The `alt` it was written with.
    pub alt: String,
}

/// [`from_html`] for a draft of this program's: each `<img src="cid:…">` that `claim` takes (it
/// answers the id the picture has in the text) stands in the text where it was, as a picture
/// run. Any other picture is left out, as in [`from_html`].
///
/// The HTML is the owner's own, so a link keeps any address [`safe_link`] accepts (a link to
/// `ms-settings:` they wrote comes back); [`from_html`] holds to what a reader follows. Never for
/// received mail.
pub fn from_html_with_pictures(html: &str, claim: &mut dyn FnMut(&ImgRef) -> Option<u32>) -> RichText {
    read(&mark_pictures(html, claim), true)
}

/// `html` with each picture `claim` takes written as `OBJECT id OBJECT` in its place, and no
/// other `OBJECT` (nothing but this puts one in the text).
fn mark_pictures(html: &str, claim: &mut dyn FnMut(&ImgRef) -> Option<u32>) -> String {
    let html = html.replace(OBJECT, "");
    let lower = html.to_ascii_lowercase();
    let (mut out, mut at) = (String::with_capacity(html.len()), 0usize);
    while let Some(start) = lower.get(at..).and_then(|rest| rest.find("<img")).map(|rel| at + rel) {
        let after = start + 4;
        let boundary = html.as_bytes().get(after).is_some_and(|b| b.is_ascii_whitespace() || matches!(b, b'/' | b'>'));
        let end = if boundary { tag_end(&html, after) } else { None };
        let Some(end) = end else {
            // Not a tag of its own (`<imgx>`) or never closed: kept as it is.
            out.push_str(html.get(at..after).unwrap_or(""));
            at = after;
            continue;
        };
        out.push_str(html.get(at..start).unwrap_or(""));
        let tag = html.get(after..end).unwrap_or("");
        match img_ref(tag).and_then(|img| claim(&img)) {
            Some(id) => out.push_str(&format!("{OBJECT}{id}{OBJECT}")),
            None => out.push_str(html.get(start..end).unwrap_or("")),
        }
        at = end;
    }
    out.push_str(html.get(at..).unwrap_or(""));
    out
}

/// Where the tag whose attributes start at `from` ends (just past its `>`), quotes respected.
fn tag_end(html: &str, from: usize) -> Option<usize> {
    let mut quote = None;
    for (i, b) in html.as_bytes().get(from..)?.iter().enumerate() {
        match (quote, *b) {
            (Some(q), b) if b == q => quote = None,
            (Some(_), _) => {}
            (None, b'"' | b'\'') => quote = Some(*b),
            (None, b'>') => return Some(from + i + 1),
            _ => {}
        }
    }
    None
}

/// The `src`, `width` and `alt` of an `<img>` whose attributes are `attrs`, when `src` is a `cid:`.
fn img_ref(attrs: &str) -> Option<ImgRef> {
    let chars: Vec<char> = attrs.trim_end_matches('>').chars().collect();
    let (mut i, mut img, mut src) = (0usize, ImgRef::default(), None);
    while i < chars.len() {
        while chars.get(i).is_some_and(|c| c.is_whitespace() || *c == '/') {
            i += 1;
        }
        let name_start = i;
        while chars.get(i).is_some_and(|c| !c.is_whitespace() && !matches!(c, '=' | '/')) {
            i += 1;
        }
        let name: String = chars.get(name_start..i).unwrap_or(&[]).iter().collect::<String>().to_ascii_lowercase();
        while chars.get(i).is_some_and(|c| c.is_whitespace()) {
            i += 1;
        }
        let mut value = String::new();
        if chars.get(i) == Some(&'=') {
            i += 1;
            while chars.get(i).is_some_and(|c| c.is_whitespace()) {
                i += 1;
            }
            match chars.get(i) {
                Some(q @ ('"' | '\'')) => {
                    i += 1;
                    while let Some(c) = chars.get(i).filter(|c| *c != q) {
                        value.push(*c);
                        i += 1;
                    }
                    i += 1;
                }
                _ => {
                    while let Some(c) = chars.get(i).filter(|c| !c.is_whitespace()) {
                        value.push(*c);
                        i += 1;
                    }
                }
            }
        }
        match name.as_str() {
            "src" => src = Some(value),
            "width" => img.width = value.trim().parse().ok(),
            "alt" => img.alt = value.replace("&quot;", "\"").replace("&#39;", "'").replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&"),
            _ => {}
        }
    }
    let src = src?;
    let src = src.trim();
    let cid = src.get(..4).filter(|p| p.eq_ignore_ascii_case("cid:")).and_then(|_| src.get(4..))?;
    img.cid = cid.to_string();
    Some(img)
}

fn read(html: &str, pictures: bool) -> RichText {
    // The look and the text only: pictures written into the HTML are not decoded for this.
    let blocks = if pictures { mailcraft_html::parse_text_own(html, safe_link) } else { mailcraft_html::parse_text(html) };
    let mut reader = Reader { out: Vec::new(), pictures };
    reader.blocks(&blocks);
    let (lead, trail) = edge_blank_lines(html);
    let found = !reader.out.is_empty();
    let mut out: Vec<Paragraph> = std::iter::repeat_with(Paragraph::default).take(lead).collect();
    out.append(&mut reader.out);
    if found {
        out.extend(std::iter::repeat_with(Paragraph::default).take(trail));
    }
    RichText::from_paragraphs(out)
}

/// The empty paragraphs [`to_html`] wrote before the first paragraph with text and after the last.
fn edge_blank_lines(html: &str) -> (usize, usize) {
    const BLANK: &str = "<div><br></div>";
    let start = html.find("<body>").map_or(0, |at| at + "<body>".len());
    let end = html.rfind("</body>").unwrap_or(html.len());
    let Some(inner) = html.get(start..end.max(start)) else { return (0, 0) };
    let mut lead = 0;
    let mut rest = inner.trim_start();
    while let Some(r) = rest.strip_prefix(BLANK) {
        lead += 1;
        rest = r.trim_start();
    }
    let mut trail = 0;
    let mut rest = inner.trim_end();
    while let Some(r) = rest.strip_suffix(BLANK) {
        trail += 1;
        rest = r.trim_end();
    }
    (lead, trail)
}

struct Reader {
    out: Vec<Paragraph>,
    /// The text holds pictures written as `OBJECT id OBJECT` ([`mark_pictures`]).
    pictures: bool,
}

impl Reader {
    fn blocks(&mut self, blocks: &[Block]) {
        for b in blocks {
            if self.out.len() >= mailcraft_model::rich::MAX_PARAGRAPHS {
                return;
            }
            match &b.kind {
                BlockKind::Table(table) => {
                    self.gap(b.space);
                    for cell in table.rows.iter().flat_map(|r| &r.cells) {
                        self.blocks(&cell.blocks);
                    }
                }
                BlockKind::Rule | BlockKind::Image { .. } => {}
                kind => {
                    // A nested list keeps its level, as deep as the model goes.
                    let (list, level) = match kind {
                        BlockKind::ListItem { marker: Marker::Bullet, depth } => (ListKind::Bullet, *depth),
                        BlockKind::ListItem { marker: Marker::Number(_) | Marker::Label(_), depth } => (ListKind::Numbered, *depth),
                        _ => (ListKind::None, 0),
                    };
                    self.gap(b.space);
                    self.paragraph(&b.spans, list, level.min(MAX_LEVEL));
                }
            }
        }
    }

    /// The blank lines above a block, as empty paragraphs (none above the first).
    fn gap(&mut self, space: u8) {
        if self.out.is_empty() {
            return;
        }
        for _ in 0..space {
            self.out.push(Paragraph::default());
        }
    }

    fn paragraph(&mut self, spans: &[Span], list: ListKind, level: u8) {
        let mut cur: Vec<Run> = Vec::new();
        for span in spans {
            let style = Style {
                bold: span.bold,
                italic: span.italic,
                underline: span.underline,
                size: span.size.map(px_to_points),
                link: span.link.as_deref().and_then(safe_link),
                color: span.color.map(|c| format!("#{:02x}{:02x}{:02x}", c.0, c.1, c.2)),
                font: span.font.as_deref().and_then(Font::from_css),
            };
            let text = span.text.replace('\u{a0}', " ");
            for (i, line) in text.split('\n').enumerate() {
                if i > 0 {
                    self.out.push(Paragraph::new(std::mem::take(&mut cur), list).at_level(level));
                }
                if self.pictures && line.contains(OBJECT) {
                    // (Text, a picture's id, text, an id, text…: the marks come in pairs.)
                    for (n, part) in line.split(OBJECT).enumerate() {
                        match (n % 2, part.parse::<u32>()) {
                            (1, Ok(id)) => cur.push(Run::picture(id, style.clone())),
                            (1, Err(_)) => {}
                            _ if part.is_empty() => {}
                            _ => cur.push(Run::new(part, style.clone())),
                        }
                    }
                    continue;
                }
                cur.push(Run::new(line, style.clone()));
            }
        }
        self.out.push(Paragraph::new(cur, list).at_level(level));
    }
}

/// CSS pixels to points, within the model's sizes.
fn px_to_points(px: u8) -> u8 {
    let points = (u16::from(px) * 3 + 2) / 4;
    u8::try_from(points).unwrap_or(MAX_SIZE).clamp(MIN_SIZE, MAX_SIZE)
}

#[cfg(test)]
mod tests;
