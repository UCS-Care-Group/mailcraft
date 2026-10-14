//! Formatted text as the HTML of a message, and back.
//!
//! [`to_html`] writes a body that was written with formatting ([`Styles`] over its text) as
//! simple HTML: one `<div>` per paragraph, `<ul>`/`<ol>` for lists, `<blockquote>` with no bar
//! for an indent, `<b>`, `<i>`, `<u>`, `<s>`, `<code>` and a `<span>` with inline `style` for the
//! rest, `<a href>` for links and `<img src="cid:…">` for pictures attached to the message.
//! Every character of the text is escaped, and nothing but those elements is ever written.
//! [`to_text`] writes the plain-text form that goes beside it.
//!
//! [`from_blocks`] (and [`from_html`]) go the other way, from display blocks of
//! `mailcraft_html` (so from any HTML, a stranger's included: only what the blocks carry
//! survives) to formatted text: that is how a draft is opened again, and how a message that is
//! replied to is quoted with its formatting. What [`to_html`] writes comes back as it was, but
//! for leading and trailing blank lines and more than three blank lines in a row.

use mailcraft_html::{Block, BlockKind, Marker, Span, bidi, escape, is_embedded, parse};
use mailcraft_model::Rgb;
use mailcraft_model::rich::{self, Align, CharStyle, List, PICTURE, ParaStyle, Run, Styles, is_link};

/// The opening of an indent: a quote with no bar, as mail programs write one.
const INDENT_OPEN: &str = "<blockquote style=\"margin:0 0 0 40px;border:none;padding:0\">";

/// The opening of a list.
fn list_open(list: List) -> &'static str {
    match list {
        List::Number => "<ol style=\"margin-top:0;margin-bottom:0\">",
        _ => "<ul style=\"margin-top:0;margin-bottom:0\">",
    }
}

fn list_close(list: List) -> &'static str {
    match list {
        List::Number => "</li></ol>",
        _ => "</li></ul>",
    }
}

/// The body of a formatted text as HTML.
pub fn to_html(text: &str, styles: &Styles) -> String {
    let mut styles = styles.clone();
    styles.normalize(text);
    let mut out = String::with_capacity(text.len() * 2 + 200);
    out.push_str("<html><head><meta charset=\"utf-8\"></head><body>\n<div style=\"font-family:Arial,Helvetica,sans-serif\">\n");
    // Lists open now, outermost first; each has an item open.
    let mut lists: Vec<List> = Vec::new();
    // How the item open in each list is aligned: an item inside it would inherit that.
    let mut aligns: Vec<Align> = Vec::new();
    let mut quotes = 0u8;
    for para in styles.paragraphs(text) {
        let style = para.style;
        let plain = para.text();
        if style.list != List::None && !style.rule {
            while quotes > 0 {
                out.push_str("</blockquote>");
                quotes -= 1;
            }
            let depth = usize::from(style.indent.min(rich::MAX_INDENT)) + 1;
            while lists.len() > depth || (lists.len() == depth && lists.last() != Some(&style.list)) {
                if let Some(l) = lists.pop() {
                    out.push_str(list_close(l));
                }
                aligns.pop();
            }
            if lists.len() == depth {
                out.push_str("</li>");
                aligns.pop();
            }
            while lists.len() < depth {
                // A level that is skipped has an item with no marker to hold the deeper one.
                out.push_str(list_open(style.list));
                lists.push(style.list);
                if lists.len() < depth {
                    out.push_str("<li style=\"list-style-type:none\">");
                    // It sits as the item around it does.
                    aligns.push(aligns.last().copied().unwrap_or_default());
                }
            }
            let inherited = aligns.last().is_some_and(|a| *a != Align::Left);
            out.push_str(&format!("<li{}>", block_attrs(style.align, &plain, inherited)));
            aligns.push(style.align);
            runs_html(&mut out, &para.runs);
            out.push('\n');
            continue;
        }
        while let Some(l) = lists.pop() {
            out.push_str(list_close(l));
        }
        aligns.clear();
        let indent = style.indent.min(rich::MAX_INDENT);
        while quotes > indent {
            out.push_str("</blockquote>");
            quotes -= 1;
        }
        while quotes < indent {
            out.push_str(INDENT_OPEN);
            quotes += 1;
        }
        if style.rule {
            out.push_str("<hr>\n");
        } else if para.runs.iter().all(|(t, s)| t.chars().all(|c| c == PICTURE && s.picture.is_none())) {
            out.push_str("<div><br></div>\n");
        } else {
            out.push_str(&format!("<div{}>", block_attrs(style.align, &plain, false)));
            runs_html(&mut out, &para.runs);
            out.push_str("</div>\n");
        }
    }
    while let Some(l) = lists.pop() {
        out.push_str(list_close(l));
    }
    while quotes > 0 {
        out.push_str("</blockquote>");
        quotes -= 1;
    }
    out.push_str("</div>\n</body></html>\n");
    out
}

/// The attributes of a paragraph: its alignment, and its direction when it runs right to left.
/// `inherited`: the paragraph is inside one that is aligned otherwise, so even the left is said.
fn block_attrs(align: Align, text: &str, inherited: bool) -> String {
    let mut out = String::new();
    if bidi::is_rtl(text) {
        out.push_str(" dir=\"rtl\"");
    }
    match align {
        Align::Left if inherited => out.push_str(" style=\"text-align:left\""),
        Align::Left => {}
        Align::Center => out.push_str(" style=\"text-align:center\""),
        Align::Right => out.push_str(" style=\"text-align:right\""),
    }
    out
}

/// The runs of one paragraph as HTML.
fn runs_html(out: &mut String, runs: &[(&str, &CharStyle)]) {
    // Where white space would collapse, it is written as no-break spaces.
    let mut after_space = true;
    let last_text = runs.iter().rposition(|(t, s)| s.picture.is_none() && t.chars().any(|c| c != PICTURE));
    for (index, (text, style)) in runs.iter().enumerate() {
        if let Some(cid) = &style.picture {
            for c in text.chars() {
                if c == PICTURE {
                    out.push_str(&format!("<img src=\"cid:{}\" alt=\"\" style=\"max-width:100%\">", escape(cid)));
                    after_space = false;
                }
            }
            continue;
        }
        let mut body = String::with_capacity(text.len() + 8);
        let count = text.chars().count();
        for (i, c) in text.chars().enumerate() {
            let last = Some(index) == last_text && i + 1 == count;
            match c {
                ' ' if after_space || last => body.push_str("&nbsp;"),
                ' ' => {
                    body.push(' ');
                    after_space = true;
                    continue;
                }
                '\t' => body.push_str("&nbsp;&nbsp;&nbsp;&nbsp;"),
                PICTURE => continue,
                c if c.is_control() => continue,
                '&' => body.push_str("&amp;"),
                '<' => body.push_str("&lt;"),
                '>' => body.push_str("&gt;"),
                '"' => body.push_str("&quot;"),
                c => body.push(c),
            }
            after_space = c == ' ' || c == '\t';
        }
        if body.is_empty() {
            continue;
        }
        out.push_str(&wrap(&body, style));
    }
}

/// Escaped text in the elements that give it `style`.
fn wrap(body: &str, style: &CharStyle) -> String {
    let mut open = String::new();
    let mut close = String::new();
    let mut tag = |o: &str, c: &str| {
        open.push_str(o);
        close.insert_str(0, c);
    };
    if let Some(link) = style.link.as_deref().filter(|l| is_link(l)) {
        tag(&format!("<a href=\"{}\">", escape(link.trim())), "</a>");
    }
    let mut css = Vec::new();
    if let Some(c) = style.color {
        css.push(format!("color:{}", hex(c)));
    }
    if let Some(c) = style.highlight {
        css.push(format!("background-color:{}", hex(c)));
    }
    if let Some(size) = style.size {
        css.push(format!("font-size:{}px", size.clamp(rich::MIN_SIZE, rich::MAX_SIZE)));
    }
    if style.fixed {
        css.push("font-family:'Courier New',monospace".to_string());
    }
    if !css.is_empty() {
        tag(&format!("<span style=\"{}\">", css.join(";")), "</span>");
    }
    if style.bold {
        tag("<b>", "</b>");
    }
    if style.italic {
        tag("<i>", "</i>");
    }
    if style.underline {
        tag("<u>", "</u>");
    }
    if style.strike {
        tag("<s>", "</s>");
    }
    if style.fixed {
        tag("<code>", "</code>");
    }
    format!("{open}{body}{close}")
}

fn hex(c: Rgb) -> String {
    format!("#{:02x}{:02x}{:02x}", c.0, c.1, c.2)
}

/// The plain-text form of a formatted text: list markers written out, indents as spaces, a
/// link's address after its text, pictures left out.
pub fn to_text(text: &str, styles: &Styles) -> String {
    let mut styles = styles.clone();
    styles.normalize(text);
    let mut lines: Vec<String> = Vec::new();
    // The next number at each depth of a numbered list.
    let mut numbers: Vec<u32> = Vec::new();
    for para in styles.paragraphs(text) {
        let style = para.style;
        let indent = usize::from(style.indent.min(rich::MAX_INDENT));
        if style.rule {
            numbers.clear();
            lines.push("________________________________".to_string());
            continue;
        }
        let mut line = "    ".repeat(indent);
        match style.list {
            List::None => numbers.clear(),
            List::Bullet => {
                numbers.truncate(indent);
                line.push_str("• ");
            }
            List::Number => {
                numbers.truncate(indent + 1);
                numbers.resize(indent + 1, 0);
                let n = numbers.last_mut().map_or(1, |n| {
                    *n = n.saturating_add(1);
                    *n
                });
                line.push_str(&format!("{n}. "));
            }
        }
        for (piece, style) in &para.runs {
            let words: String = piece.chars().filter(|c| *c != PICTURE && (!c.is_control() || *c == '\t')).collect();
            line.push_str(&words);
            if let Some(link) = style.link.as_deref().filter(|l| is_link(l))
                && words.trim() != link.trim()
                && !link.get(7..).is_some_and(|rest| link.starts_with("mailto:") && rest == words.trim())
            {
                line.push_str(&format!(" <{}>", link.trim()));
            }
        }
        lines.push(line.trim_end().to_string());
    }
    lines.join("\n")
}

/// Formatted text from HTML, by way of its display blocks.
pub fn from_html(html: &str) -> (String, Styles) {
    from_blocks(&parse(html))
}

/// Formatted text from display blocks: their text, the whitelisted look of their spans, their
/// alignment, lists and quoting (as an indent). Tables are read cell by cell, a picture attached
/// to the message stays one, a picture from the web leaves its description.
pub fn from_blocks(blocks: &[Block]) -> (String, Styles) {
    let mut out = Out::default();
    walk(&mut out, blocks, 0);
    let text = out.lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n");
    let mut runs: Vec<Run> = Vec::new();
    for (i, line) in out.lines.iter().enumerate() {
        if i > 0 {
            runs.push(Run { len: 1, style: CharStyle::default() });
        }
        runs.extend(line.runs.iter().cloned());
    }
    let mut styles = Styles { runs, paras: out.lines.iter().map(|l| l.para).collect() };
    styles.normalize(&text);
    (text, styles)
}

#[derive(Default)]
struct Line {
    text: String,
    runs: Vec<Run>,
    para: ParaStyle,
}

#[derive(Default)]
struct Out {
    lines: Vec<Line>,
    /// The last block read was a list item: of what kind, how deep.
    in_list: Option<(List, u8)>,
    /// The kind of the list at the top level that is being read.
    top: Option<List>,
}

impl Out {
    fn blank(&mut self, n: u8) {
        if self.lines.is_empty() {
            return;
        }
        for _ in 0..n {
            self.lines.push(Line::default());
        }
    }
}

fn walk(out: &mut Out, blocks: &[Block], quote: u8) {
    for block in blocks {
        let depth = quote.saturating_add(block.quote);
        let item = match &block.kind {
            BlockKind::ListItem { marker: Marker::Bullet, depth } => Some((List::Bullet, *depth)),
            BlockKind::ListItem { marker: Marker::Number(_) | Marker::Label(_), depth } => Some((List::Number, *depth)),
            BlockKind::ListItem { marker: Marker::None, depth } => Some((List::None, *depth)),
            _ => None,
        };
        // A list starts with a gap of its own that writing it adds back: after other text, and
        // where a list of the other kind follows one at the top level.
        let new_list = match (item, out.in_list) {
            (Some(_), None) => true,
            (Some((kind, 0)), Some(_)) => out.top.is_some_and(|top| top != kind) && kind != List::None,
            _ => false,
        };
        match item {
            Some((kind, depth)) if kind != List::None && (depth == 0 || out.in_list.is_none()) => out.top = Some(kind),
            Some(_) => {}
            None => out.top = None,
        }
        let space = if new_list { block.space.saturating_sub(1) } else { block.space };
        out.blank(if out.in_list.is_some() && item.is_none() { space.saturating_sub(1) } else { space });
        out.in_list = item;
        let align = match block.align {
            mailcraft_html::Align::Left => Align::Left,
            mailcraft_html::Align::Center => Align::Center,
            mailcraft_html::Align::Right => Align::Right,
        };
        let indent = depth.min(rich::MAX_INDENT);
        match &block.kind {
            BlockKind::Rule => out.lines.push(Line { para: ParaStyle { rule: true, indent, ..ParaStyle::default() }, ..Line::default() }),
            BlockKind::Image { src, .. } => {
                if is_embedded(src) {
                    let cid = rich::clean_content_id(src.trim().get(4..).unwrap_or(""));
                    if !cid.is_empty() {
                        let style = CharStyle { picture: Some(cid), ..CharStyle::default() };
                        out.lines.push(Line {
                            text: PICTURE.to_string(),
                            runs: vec![Run { len: 1, style }],
                            para: ParaStyle { align, indent, ..ParaStyle::default() },
                        });
                        continue;
                    }
                }
                let alt = block.text();
                let alt = alt.trim();
                if !alt.is_empty() {
                    let style = CharStyle::default();
                    let text = clean(&format!("[{alt}]"));
                    out.lines.push(Line {
                        runs: vec![Run { len: text.chars().count(), style }],
                        text,
                        para: ParaStyle { align, indent, ..ParaStyle::default() },
                    });
                }
            }
            BlockKind::Table(table) => {
                for cell in table.rows.iter().flat_map(|r| &r.cells) {
                    walk(out, &cell.blocks, depth);
                }
                out.in_list = None;
            }
            kind => {
                let (list, list_depth, heading) = match kind {
                    BlockKind::ListItem { marker: Marker::Bullet, depth } => (List::Bullet, *depth, None),
                    BlockKind::ListItem { marker: Marker::Number(_) | Marker::Label(_), depth } => (List::Number, *depth, None),
                    BlockKind::ListItem { marker: Marker::None, depth } => (List::None, depth.saturating_add(1), None),
                    BlockKind::Heading(level) => (List::None, 0, Some(*level)),
                    _ => (List::None, 0, None),
                };
                let pre = matches!(kind, BlockKind::Preformatted);
                let indent = depth.saturating_add(list_depth).min(rich::MAX_INDENT);
                let para = ParaStyle { list, indent, align, rule: false };
                let mut line = Line { para, ..Line::default() };
                for span in &block.spans {
                    let style = span_style(span, heading, pre);
                    for (i, piece) in span.text.split('\n').enumerate() {
                        if i > 0 {
                            // A line break inside the block: the next line goes on under it.
                            let next = ParaStyle { list: List::None, ..para };
                            out.lines.push(std::mem::replace(&mut line, Line { para: next, ..Line::default() }));
                        }
                        let piece = clean(piece);
                        if piece.is_empty() {
                            continue;
                        }
                        let len = piece.chars().count();
                        line.text.push_str(&piece);
                        line.runs.push(Run { len, style: style.clone() });
                    }
                }
                out.lines.push(line);
            }
        }
    }
}

/// Text from a block as it can be written: a no-break space is a space, and no control
/// character or picture mark survives.
fn clean(text: &str) -> String {
    text.chars().filter(|c| !c.is_control() && *c != PICTURE).map(|c| if c == '\u{a0}' { ' ' } else { c }).collect()
}

/// How a span of a block looks, as formatted text has it.
fn span_style(span: &Span, heading: Option<u8>, pre: bool) -> CharStyle {
    let heading_size = match heading {
        Some(1) => Some(24),
        Some(2) => Some(20),
        Some(3) => Some(16),
        _ => None,
    };
    CharStyle {
        bold: span.bold || heading.is_some(),
        italic: span.italic,
        underline: span.underline,
        strike: span.strike,
        fixed: span.code || pre,
        size: span.size.or(heading_size).map(|s| s.clamp(rich::MIN_SIZE, rich::MAX_SIZE)),
        color: span.color.map(|c| Rgb(c.0, c.1, c.2)),
        highlight: span.background.map(|c| Rgb(c.0, c.1, c.2)),
        link: span.link.clone().filter(|l| is_link(l)),
        picture: None,
    }
}

#[cfg(test)]
mod tests {
    use mailcraft_model::rich::{MAX_SIZE, MIN_SIZE};
    use proptest::prelude::*;

    use super::*;

    fn styled(text: &str, f: impl FnOnce(&mut Styles)) -> Styles {
        let mut s = Styles::plain(text);
        f(&mut s);
        s.normalize(text);
        s
    }

    /// Where `needle` is in `text`, in characters.
    fn at(text: &str, needle: &str) -> std::ops::Range<usize> {
        let start = text.find(needle).map_or(0, |b| text[..b].chars().count());
        start..start + needle.chars().count()
    }

    fn round_trip(text: &str, styles: &Styles) -> (String, Styles) {
        from_html(&to_html(text, styles))
    }

    #[test]
    fn formatting_is_written_as_simple_html_and_comes_back() {
        let text = "Hello bold world\nsecond  line\n\ncentred";
        let s = styled(text, |s| {
            s.apply(text, 6..10, |c| c.bold = true);
            s.apply(text, 11..16, |c| {
                c.italic = true;
                c.color = Some(Rgb(0xC0, 0x10, 0x20));
                c.size = Some(20);
            });
            s.apply(text, 17..23, |c| c.highlight = Some(Rgb(255, 255, 0)));
            s.apply_paras(text, 32..32, |p| p.align = Align::Center);
        });
        let html = to_html(text, &s);
        assert!(html.contains("<b>bold</b>"), "{html}");
        assert!(html.contains("<span style=\"color:#c01020;font-size:20px\"><i>world</i></span>"), "{html}");
        assert!(html.contains("</span> &nbsp;line"), "{html}");
        assert!(html.contains("<div style=\"text-align:center\">centred</div>"), "{html}");
        assert_eq!(round_trip(text, &s), (text.to_string(), s));
    }

    #[test]
    fn lists_indents_rules_links_and_pictures_come_back() {
        let text = "Intro\none\ntwo\nnested\nthree\nafter\nindented\n\nlink here\n\u{FFFC}\nend";
        let s = styled(text, |s| {
            s.paras[1].list = List::Bullet;
            s.paras[2].list = List::Bullet;
            s.paras[3].list = List::Number;
            s.paras[3].indent = 1;
            s.paras[4].list = List::Bullet;
            s.paras[6].indent = 2;
            s.paras[7].rule = true;
            s.apply(text, at(text, "link"), |c| c.link = Some("https://example.org/a?b=1&c=\"2\"".into()));
            s.apply(text, at(text, "\u{FFFC}"), |c| c.picture = Some("pic1@mailcraft".into()));
        });
        let html = to_html(text, &s);
        assert!(html.contains("<ul style=\"margin-top:0;margin-bottom:0\"><li>one"), "{html}");
        assert!(html.contains("<ol style=\"margin-top:0;margin-bottom:0\"><li>nested"), "{html}");
        assert!(html.contains("<img src=\"cid:pic1@mailcraft\""), "{html}");
        assert!(html.contains("href=\"https://example.org/a?b=1&amp;c=&quot;2&quot;\""), "{html}");
        assert_eq!(round_trip(text, &s), (text.to_string(), s.clone()));
        let plain = to_text(text, &s);
        assert!(plain.contains("• one\n• two\n    1. nested\n• three\nafter\n        indented"), "{plain}");
        assert!(plain.contains("link <https://example.org/a?b=1&c=\"2\">"), "{plain}");
        assert!(!plain.contains('\u{FFFC}'));
    }

    #[test]
    fn everything_is_escaped() {
        let text = "<script>alert('x')</script> & \"q\"";
        let s = styled(text, |s| s.apply(text, 0..8, |c| c.link = Some("javascript:alert(1)".into())));
        let html = to_html(text, &s);
        assert!(!html.contains("<script") && !html.contains("javascript:"), "{html}");
        assert_eq!(from_html(&html).0, text);
    }

    #[test]
    fn right_to_left_paragraphs_say_so() {
        let text = "שלום עולם\nHello";
        let html = to_html(text, &Styles::plain(text));
        assert!(html.contains("<div dir=\"rtl\">שלום עולם</div>"), "{html}");
        assert!(html.contains("<div>Hello</div>"), "{html}");
    }

    #[test]
    fn a_strangers_html_keeps_only_what_the_blocks_carry() {
        let (text, s) = from_html(
            "<h1 onclick=x>Title</h1><p style=\"color:#123456;position:fixed\">A <a href=\"javascript:x\">bad</a> and <a href=\"https://example.org\">good</a> link</p>\
             <table><tr><td>cell one</td><td><img src=\"https://tracker.example/p.gif\" alt=\"logo\"></td></tr></table><img src=\"cid:x@y\">",
        );
        assert_eq!(text, "Title\n\nA bad and good link\n\ncell one\n[logo]\n\u{FFFC}");
        assert!(s.runs.iter().all(|r| r.style.link.as_deref().is_none_or(|l| l == "https://example.org")));
        assert!(s.every(0..5, |c| c.bold && c.size == Some(24)));
        assert!(s.every(7..8, |c| c.color == Some(Rgb(0x12, 0x34, 0x56))));
        assert_eq!(s.style_of(text.chars().count() - 1).and_then(|c| c.picture.clone()).as_deref(), Some("x@y"));
    }

    fn arb_style() -> impl Strategy<Value = CharStyle> {
        (
            any::<[bool; 5]>(),
            proptest::option::of(MIN_SIZE..=MAX_SIZE),
            proptest::option::of(any::<(u8, u8, u8)>()),
            proptest::option::of(any::<(u8, u8, u8)>()),
            proptest::option::of(prop_oneof![Just("https://example.org/x".to_string()), Just("mailto:ada@example.org".to_string())]),
        )
            .prop_map(|(b, size, color, highlight, link)| CharStyle {
                bold: b[0],
                italic: b[1],
                underline: b[2],
                strike: b[3],
                fixed: b[4],
                size,
                color: color.map(|(r, g, b)| Rgb(r, g, b)),
                highlight: highlight.map(|(r, g, b)| Rgb(r, g, b)),
                link,
                picture: None,
            })
    }

    fn arb_para() -> impl Strategy<Value = ParaStyle> {
        (0u8..3, 0u8..3, 0u8..3).prop_map(|(list, indent, align)| ParaStyle {
            list: [List::None, List::Bullet, List::Number][usize::from(list)],
            indent,
            align: [Align::Left, Align::Center, Align::Right][usize::from(align)],
            rule: false,
        })
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(200))]

        /// Whatever is written re-parses, has no markup of the text in it, and gives back the
        /// same text (and the same look, wherever a reader can tell).
        #[test]
        fn rich_bodies_round_trip(
            lines in proptest::collection::vec(("[a-zA-Z<>&\"' ]{1,12}", arb_para(), proptest::collection::vec((1usize..6, arb_style()), 0..4)), 1..8)
        ) {
            // Paragraphs that a reader can see: text with a letter in, not only spaces.
            let lines: Vec<_> = lines.into_iter().map(|(t, p, r)| (format!("x{}x", t), p, r)).collect();
            let text = lines.iter().map(|(t, _, _)| t.as_str()).collect::<Vec<_>>().join("\n");
            let mut runs = Vec::new();
            for (i, (t, _, r)) in lines.iter().enumerate() {
                if i > 0 { runs.push(Run { len: 1, style: CharStyle::default() }); }
                let mut left = t.chars().count();
                for (len, style) in r {
                    let len = (*len).min(left);
                    left -= len;
                    runs.push(Run { len, style: style.clone() });
                }
                runs.push(Run { len: left, style: CharStyle::default() });
            }
            let mut styles = Styles { runs, paras: lines.iter().map(|(_, p, _)| *p).collect() };
            styles.normalize(&text);
            let html = to_html(&text, &styles);
            prop_assert!(!html.contains("<script"));
            let (back, back_styles) = from_html(&html);
            prop_assert_eq!(&back, &text);
            // Underline on a link can't be told from the link's own.
            let look = |s: &Styles, t: &str| s.paragraphs(t).iter().map(|p| (p.style, p.runs.iter().map(|(x, c)| (x.to_string(), (*c).clone())).collect::<Vec<_>>())).collect::<Vec<_>>();
            prop_assert_eq!(look(&back_styles, &back), look(&styles, &text));
            let plain = to_text(&text, &styles);
            prop_assert_eq!(plain.lines().count(), lines.len());
        }

        #[test]
        fn any_styles_on_any_text_never_panic(text in "\\PC{0,40}", runs in proptest::collection::vec((0usize..50, arb_style()), 0..6), paras in proptest::collection::vec(arb_para(), 0..6)) {
            let styles = Styles { runs: runs.into_iter().map(|(len, style)| Run { len, style }).collect(), paras };
            let html = to_html(&text, &styles);
            let _ = to_text(&text, &styles);
            let _ = from_html(&html);
        }

        #[test]
        fn any_html_reads_as_formatted_text(html in "\\PC{0,200}") {
            let (text, styles) = from_html(&html);
            prop_assert_eq!(styles.runs.iter().map(|r| r.len).sum::<usize>(), text.chars().count());
            prop_assert_eq!(styles.paras.len(), text.split('\n').count());
        }
    }
}
