// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;
use crate::{MAX_FONT_PX, MIN_FONT_PX};

#[test]
fn a_list_without_margins_has_no_gap_around_it() {
    let spaces = |html: &str| parse(html).iter().map(|b| b.space).collect::<Vec<_>>();
    assert_eq!(spaces("<div>a</div><ul><li>b</li><li>c</li></ul><div>d</div>"), [0, 1, 0, 1]);
    assert_eq!(spaces("<div>a</div><ul style=\"margin-top:0;margin-bottom:0\"><li>b</li></ul><div>d</div>"), [0, 0, 0]);
    assert_eq!(spaces("<div>a</div><ol style=\"margin:0\"><li>b</li></ol><div>d</div>"), [0, 0, 0]);
    assert_eq!(spaces("<div>a</div><ul style=\"margin-top:0in\"><li>b</li></ul><div>d</div>"), [0, 0, 1]);
    // A blank line before such a list is the owner's, and stays.
    assert_eq!(spaces("<div>a</div><div><br></div><ul style=\"margin:0\"><li>b</li></ul>"), [0, 1]);
}

fn para(text: &str) -> Block {
    Block::new(BlockKind::Paragraph, vec![Span::plain(text)])
}

fn hex(c: Rgb) -> String {
    format!("{:02x}{:02x}{:02x}", c.0, c.1, c.2)
}

/// A span's text with its formatting in front: `[B#ff0000^16:text→url]`.
fn span_text(s: &Span) -> String {
    let mut marks = String::new();
    for (on, mark) in [(s.bold, 'B'), (s.italic, 'I'), (s.underline, 'U'), (s.strike, 'S'), (s.code, 'C')] {
        if on {
            marks.push(mark);
        }
    }
    if let Some(c) = s.color {
        marks.push_str(&format!("#{}", hex(c)));
    }
    if let Some(c) = s.background {
        marks.push_str(&format!("@{}", hex(c)));
    }
    if let Some(size) = s.size {
        marks.push_str(&format!("^{size}"));
    }
    match (&s.link, marks.is_empty()) {
        (Some(url), _) => format!("[{marks}:{}→{url}]", s.text),
        (None, false) => format!("[{marks}:{}]", s.text),
        (None, true) => s.text.clone(),
    }
}

/// One block as `kind|text` (`kind|q1|text` when quoted, `kind.c` centred, `kind.r` right-aligned).
/// A table is `data[(cell)(cell)][(cell)]` or `layout…`, with `.c`, `+border`, `+outline`, `w=…`
/// and `bg=…` after the word, a cell's background as `(@rrggbb …)`, and its blocks joined by ` ; `.
fn dump_block(b: &Block) -> String {
    let align = match b.align {
        Align::Left => "",
        Align::Center => ".c",
        Align::Right => ".r",
    };
    let kind = match &b.kind {
        BlockKind::Paragraph => "p".to_string(),
        BlockKind::Heading(n) => format!("h{n}"),
        BlockKind::ListItem { marker: Marker::Number(n), depth } => format!("ol{depth}#{n}"),
        BlockKind::ListItem { marker: Marker::Label(l), depth } => format!("ol{depth}#{l}"),
        BlockKind::ListItem { marker: Marker::Bullet, depth } => format!("ul{depth}"),
        BlockKind::ListItem { marker: Marker::None, depth } => format!("li{depth}"),
        BlockKind::Preformatted => "pre".to_string(),
        BlockKind::Rule => "hr".to_string(),
        BlockKind::Image { src, width, height, .. } => match (width, height) {
            (Some(w), Some(h)) => format!("img({src} {w}x{h})"),
            (Some(w), None) => format!("img({src} {w}x)"),
            _ => format!("img({src})"),
        },
        BlockKind::Table(t) => {
            let mut out = format!("{}{align}", if t.layout { "layout" } else { "data" });
            if t.border {
                out.push_str("+border");
            }
            if t.outline {
                out.push_str("+outline");
            }
            match t.width {
                Some(Length::Px(w)) => out.push_str(&format!(" w={w}")),
                Some(Length::Percent(w)) => out.push_str(&format!(" w={w}%")),
                None => {}
            }
            if let Some(bg) = t.background {
                out.push_str(&format!(" bg={}", hex(bg)));
            }
            for row in &t.rows {
                out.push('[');
                for cell in &row.cells {
                    out.push('(');
                    if cell.header {
                        out.push_str("th ");
                    }
                    if cell.colspan > 1 {
                        out.push_str(&format!("x{} ", cell.colspan));
                    }
                    if let Some(bg) = cell.background {
                        out.push_str(&format!("@{} ", hex(bg)));
                    }
                    if cell.border.any() {
                        out.push('+');
                        for (on, side) in [(cell.border.top, 't'), (cell.border.right, 'r'), (cell.border.bottom, 'b'), (cell.border.left, 'l')] {
                            if on {
                                out.push(side);
                            }
                        }
                        out.push(' ');
                    }
                    out.push_str(&cell.blocks.iter().map(dump_block).collect::<Vec<_>>().join(" ; "));
                    out.push(')');
                }
                out.push(']');
            }
            return if b.quote > 0 { format!("{out}|q{}", b.quote) } else { out };
        }
    };
    let text: String = b.spans.iter().map(span_text).collect();
    if b.quote > 0 { format!("{kind}{align}|q{}|{text}", b.quote) } else { format!("{kind}{align}|{text}") }
}

fn dump(html: &str) -> Vec<String> {
    parse(html).iter().map(dump_block).collect()
}

/// The blank lines above each top-level block.
fn spaces(html: &str) -> Vec<u8> {
    parse(html).iter().map(|b| b.space).collect()
}

/// Every text in the blocks, looking inside tables.
fn all_text(blocks: &[Block]) -> String {
    let mut out = String::new();
    for b in blocks {
        match &b.kind {
            BlockKind::Table(t) => {
                for cell in t.rows.iter().flat_map(|r| &r.cells) {
                    out.push_str(&all_text(&cell.blocks));
                }
            }
            _ => {
                out.push_str(&b.text());
                out.push('\n');
            }
        }
    }
    out
}

/// The deepest tables nest in the blocks.
fn table_depth(blocks: &[Block]) -> usize {
    blocks
        .iter()
        .map(|b| match &b.kind {
            BlockKind::Table(t) => 1 + t.rows.iter().flat_map(|r| &r.cells).map(|c| table_depth(&c.blocks)).max().unwrap_or(0),
            _ => 0,
        })
        .max()
        .unwrap_or(0)
}

fn count_cells(blocks: &[Block]) -> usize {
    blocks
        .iter()
        .map(|b| match &b.kind {
            BlockKind::Table(t) => t.rows.iter().flat_map(|r| &r.cells).map(|c| 1 + count_cells(&c.blocks)).sum(),
            _ => 0,
        })
        .sum()
}

#[test]
fn plain_text_and_paragraphs() {
    assert_eq!(parse("Hello"), [para("Hello")]);
    assert_eq!(dump("<p>One</p><p>Two</p>"), ["p|One", "p|Two"]);
    assert_eq!(dump("<div>One</div>Two<div>Three</div>"), ["p|One", "p|Two", "p|Three"]);
    assert_eq!(parse(""), []);
    assert_eq!(parse("   \n\t "), []);
}

#[test]
fn unclosed_paragraphs_still_break() {
    assert_eq!(dump("<p>One<p>Two<p>Three"), ["p|One", "p|Two", "p|Three"]);
    assert_eq!(dump("One</p>Two</div>Three"), ["p|One", "p|Two", "p|Three"]);
}

#[test]
fn white_space_collapses() {
    assert_eq!(parse("<p>  Hello \n\t  big   world  </p>"), [para("Hello big world")]);
    assert_eq!(dump("<p>a <b> b </b> c</p>"), ["p|a [B:b ]c"]);
    assert_eq!(dump("<p>a<b> b</b></p>"), ["p|a[B: b]"]);
    // No-break spaces do not collapse.
    assert_eq!(parse("<p>a&nbsp;&nbsp;b</p>"), [para("a\u{a0}\u{a0}b")]);
}

#[test]
fn blank_paragraphs_are_blank_lines_not_blocks() {
    assert_eq!(parse("<p></p><p> </p><p>&nbsp;</p><div><br></div><p>x</p><p>&zwnj;&nbsp;&#8203;</p>"), [para("x")]);
    assert_eq!(dump("<div>a</div><div><br></div><div>b</div>"), ["p|a", "p|b"]);
}

#[test]
fn lines_and_paragraphs_are_spaced_the_way_mail_programs_do_it() {
    // A paragraph has a gap above and below; lines that follow each other have none.
    assert_eq!(spaces("<p>a</p><p>b</p>"), [0, 1]);
    assert_eq!(spaces("<div>a</div><div>b</div>"), [0, 0]);
    assert_eq!(spaces("<p>a</p><div>b</div><div>c</div><p>d</p>"), [0, 1, 0, 1]);
    assert_eq!(spaces("a<p>b</p>c"), [0, 1, 1]);
    // An empty line is written as a line holding a break or a no-break space.
    assert_eq!(spaces("<div>a</div><div><br></div><div>b</div>"), [0, 1]);
    assert_eq!(spaces("<div>a</div><div>&nbsp;</div><div>&nbsp;</div><div>b</div>"), [0, 2]);
    assert_eq!(spaces("<div>a</div><br><div>b</div>"), [0, 1]);
    assert_eq!(spaces("<div>a<br><br></div><div>b</div>"), [0, 1]);
    assert_eq!(spaces("<div>a<br></div><div>b</div>"), [0, 0]);
    // Outlook's own paragraphs have no margin, and its blank line is an empty paragraph.
    let outlook = "<p class=MsoNormal>a<o:p></o:p></p><p class=MsoNormal>b</p><p class=MsoNormal><o:p>&nbsp;</o:p></p><p class=\"x MsoNormal\">c</p>";
    assert_eq!(dump(outlook), ["p|a", "p|b", "p|c"]);
    assert_eq!(spaces(outlook), [0, 0, 1]);
    // A paragraph that says it has no margin has none; one that says it has one keeps it.
    assert_eq!(spaces("<p style='margin:0'>a</p><p style='margin:0in 0in 0in 0in'>b</p><p style='margin-top:0;margin-bottom:0'>c</p>"), [0, 0, 0]);
    assert_eq!(spaces("<p class=MsoNormal style='margin-bottom:12.0pt'>a</p><p class=MsoNormal>b</p>"), [0, 1]);
    assert_eq!(spaces("<div>a</div><div style='margin-top:20px'>b</div>"), [0, 1]);
    // Never more than three blank lines, and none above the first block.
    assert_eq!(spaces("<br><br><p>a</p><br><br><br><br><br><br><p>b</p>"), [0, 3]);
    // Headings, lists, quotes and preformatted text stand apart; list items stay together.
    assert_eq!(spaces("<div>x</div><h2>T</h2><div>y</div>"), [0, 1, 1]);
    assert_eq!(spaces("<div>x</div><ul><li>a</li><li>b<ul><li>c</li></ul></li></ul><div>y</div>"), [0, 1, 0, 0, 1]);
    assert_eq!(spaces("<div>x</div><blockquote>q</blockquote><div>y</div>"), [0, 1, 1]);
    assert_eq!(spaces("<div>x</div><blockquote style='margin:0 0 0 .8ex'>q</blockquote><div>y</div>"), [0, 0, 0]);
    assert_eq!(spaces("<div>x</div><pre>code</pre><div>y</div>"), [0, 1, 1]);
}

#[test]
fn entities_decode_in_text() {
    assert_eq!(parse("<p>Tom &amp; Jerry &lt;3 &copy; 2026 &#8212; caf&eacute;</p>"), [para("Tom & Jerry <3 © 2026 — café")]);
    assert_eq!(parse("AT&T & co"), [para("AT&T & co")]);
}

#[test]
fn inline_formatting() {
    assert_eq!(dump("<b>bold</b> <i>it</i> <u>un</u> <s>st</s> <code>co</code>"), ["p|[B:bold] [I:it] [U:un] [S:st] [C:co]"]);
    assert_eq!(dump("<strong>a</strong><em>b</em><strike>c</strike><del>d</del><tt>e</tt><kbd>f</kbd>"), ["p|[B:a][I:b][S:cd][C:ef]"]);
    assert_eq!(dump("<b><i>both</i> bold</b> none"), ["p|[BI:both][B: bold] none"]);
    assert_eq!(dump("<span style='color:red'>a</span><font size=2>b</font>"), ["p|[#ff0000:a][^13:b]"]);
}

#[test]
fn adjacent_spans_with_one_format_merge() {
    assert_eq!(dump("<b>a</b><b>b</b><strong>c</strong>"), ["p|[B:abc]"]);
    assert_eq!(parse("a<span>b</span>c<i></i>d").first().map(|b| b.spans.len()), Some(1));
    assert_eq!(dump("<span style='color:#f00'>a</span><span style='color:red'>b</span>"), ["p|[#ff0000:ab]"]);
}

#[test]
fn misnested_and_stray_inline_tags() {
    assert_eq!(dump("<b>a<i>b</b>c</i>d"), ["p|[B:a][BI:b][I:c]d"]);
    assert_eq!(dump("</b></i>plain</u>"), ["p|plain"]);
    // As in a browser, an open <b> carries into the next paragraph.
    assert_eq!(dump("<p><b>a</p><p>b</b>c</p>"), ["p|[B:a]", "p|[B:b]c"]);
    // A span does not: it ends with the block it is in.
    assert_eq!(dump("<div><span style='color:red'>a</div><div>b</div>"), ["p|[#ff0000:a]", "p|b"]);
    // An end tag that would close a block opened inside the span is ignored.
    assert_eq!(dump("<span style='color:red'>a<div>b</span>c</div>d"), ["p|[#ff0000:a]", "p|[#ff0000:bc]", "p|[#ff0000:d]"]);
}

#[test]
fn links_keep_only_safe_urls() {
    assert_eq!(dump(r#"<a href="https://example.org/a?x=1&amp;y=2">go</a>"#), ["p|[:go→https://example.org/a?x=1&y=2]"]);
    assert_eq!(dump(r#"<a href=" mailto:ada@example.org ">mail</a>"#), ["p|[:mail→mailto:ada@example.org]"]);
    assert_eq!(dump(r#"<a href="javascript:alert(1)">x</a>"#), ["p|x"]);
    assert_eq!(dump(r#"<a href="JAVA&#9;SCRIPT:alert(1)">x</a>"#), ["p|x"]);
    assert_eq!(dump(r#"<a href="data:text/html,hi">x</a><a href="/rel">y</a><a name=top>z</a>"#), ["p|xyz"]);
    assert_eq!(
        dump(r#"<a href="https://example.org"><b>bold</b> link</a> after"#),
        ["p|[B:bold→https://example.org][: link→https://example.org] after"]
    );
    // A second <a> replaces the first; </a> ends the link.
    assert_eq!(dump(r#"<a href="https://a.example">a<a href="https://b.example">b</a>c"#), ["p|[:a→https://a.example][:b→https://b.example]c"]);
}

#[test]
fn a_link_has_its_own_colour_or_none() {
    // The colour around a link is not the link's, as in a browser; one it names itself is.
    assert_eq!(
        dump(r#"<span style="color:#333333">see <a href="https://example.org">this</a> now</span>"#),
        ["p|[#333333:see ][:this→https://example.org][#333333: now]"]
    );
    assert_eq!(
        dump(r##"<a href="https://example.org" style="color:#ffffff;text-decoration:none">Shop</a>"##),
        ["p|[#ffffff:Shop→https://example.org]"]
    );
    assert_eq!(dump(r#"<a href="https://example.org"><span style="color:green">in</span></a>"#), ["p|[#008000:in→https://example.org]"]);
    // An anchor that leads nowhere is just text.
    assert_eq!(dump(r#"<font color=red><a name="top">x</a></font>"#), ["p|[#ff0000:x]"]);
}

#[test]
fn line_breaks() {
    assert_eq!(parse("a<br>b<br/>c"), [para("a\nb\nc")]);
    assert_eq!(parse("<p>a <br> b</p>"), [para("a\nb")]);
    assert_eq!(parse("<br><br>a<br><br>b<br><br>"), [para("a\n\nb")]);
    assert_eq!(dump("<p>a<br></p><br><p>b</p>"), ["p|a", "p|b"]);
    assert_eq!(parse("a</br>b"), [para("a\nb")]);
}

#[test]
fn headings() {
    assert_eq!(dump("<h1>Title</h1><p>text</p><h3>Sub <i>x</i></h3><h6>six</h6>"), ["h1|[B:Title]", "p|text", "h3|[B:Sub ][BI:x]", "h6|[B:six]"]);
    assert_eq!(dump("<h2>a<h3>b</h3>c"), ["h2|[B:a]", "h3|[B:b]", "p|c"]);
    assert_eq!(dump("<h2>a</h4>b"), ["h2|[B:a]", "p|b"]);
    // A heading is bold unless it says it is not.
    assert_eq!(dump("<h1 style='font-weight:normal'>light</h1><h2 style='font-weight:400'>too <b>but</b></h2>"), ["h1|light", "h2|too [B:but]"]);
    assert_eq!(dump("<h2 align=center style='color:#036'>News</h2>"), ["h2.c|[B#003366:News]"]);
}

#[test]
fn lists() {
    assert_eq!(dump("<ul><li>a</li><li>b</li></ul>"), ["ul0|a", "ul0|b"]);
    assert_eq!(dump("<ol><li>a<li>b<li>c</ol>after"), ["ol0#1|a", "ol0#2|b", "ol0#3|c", "p|after"]);
    assert_eq!(dump("<ol start=5><li>a</li><li>b</li></ol>"), ["ol0#5|a", "ol0#6|b"]);
    assert_eq!(dump("<ol start='-3'><li>a</li></ol><ol start=99999999999><li>b</li></ol>"), ["ol0#1|a", "ol0#1|b"]);
    assert_eq!(dump("<li>stray</li>"), ["ul0|stray"]);
    assert_eq!(dump("<ul><li></li><li> </li><li>x</li></ul>"), ["ul0|x"]);
}

#[test]
fn ordered_lists_count_the_way_they_are_told() {
    assert_eq!(dump("<ol type=a start=3><li>x<li>y</ol>"), ["ol0#c.|x", "ol0#d.|y"]);
    assert_eq!(dump("<ol type=A start=27><li>x</ol>"), ["ol0#AA.|x"]);
    assert_eq!(dump("<ol type=i start=3><li>x<li>y</ol>"), ["ol0#iii.|x", "ol0#iv.|y"]);
    assert_eq!(dump("<ol type=I start=1994><li>x</ol>"), ["ol0#MCMXCIV.|x"]);
    // Past what Roman numerals reach, plain numbers.
    assert_eq!(dump("<ol type=i start=4000><li>x</ol>"), ["ol0#4000|x"]);
    assert_eq!(dump("<ol><li>a<li value=7>b<li>c</ol>"), ["ol0#1|a", "ol0#7|b", "ol0#8|c"]);
    assert_eq!(dump("<ol start=4294967295><li>a<li>b</ol>"), ["ol0#4294967295|a", "ol0#4294967295|b"]);
}

#[test]
fn nested_lists() {
    let html = "<ul><li>a<ul><li>a1</li><li>a2<ol><li>deep</li></ol></li></ul></li><li>b</li></ul>";
    assert_eq!(dump(html), ["ul0|a", "ul1|a1", "ul1|a2", "ol2#1|deep", "ul0|b"]);
    assert_eq!(dump("<ol><li>one<ol><li>inner</li></ol></li><li>two</li></ol>"), ["ol0#1|one", "ol1#1|inner", "ol0#2|two"]);
    // Paragraphs inside an item: the first carries the marker, the rest stay under it.
    assert_eq!(dump("<ul><li><p>first</p><p>second</p></li><li>next</li></ul>"), ["ul0|first", "li0|second", "ul0|next"]);
    assert_eq!(dump("<ul><li>a<ul><li>b</li></ul>back</li></ul>"), ["ul0|a", "ul1|b", "li0|back"]);
}

#[test]
fn outlook_list_paragraphs_become_list_items() {
    // What Outlook writes for a bulleted and a numbered list: paragraphs with the marker as text.
    let bullet = "<p class=MsoListParagraph style='text-indent:-.25in;mso-list:l0 level1 lfo1'><![if !supportLists]>\
                  <span style='font-family:Symbol'><span style='mso-list:Ignore'>·<span style='font:7.0pt \"Times New Roman\"'>&nbsp;&nbsp;&nbsp;&nbsp;&nbsp; </span></span></span>\
                  <![endif]>First point<o:p></o:p></p>\
                  <p class=MsoListParagraph style='margin-left:1.0in;text-indent:-.25in;mso-list:l0 level2 lfo1'><![if !supportLists]>\
                  <span style='font-family:\"Courier New\"'><span style='mso-list:Ignore'>o<span style='font:7.0pt \"Times New Roman\"'>&nbsp;&nbsp; </span></span></span>\
                  <![endif]>Under it<o:p></o:p></p><p class=MsoNormal>After</p>";
    assert_eq!(dump(bullet), ["ul0|First point", "ul1|Under it", "p|After"]);
    assert_eq!(spaces(bullet), [0, 0, 0]);
    let numbered = "<p style='mso-list:l1 level1 lfo2'><span style='mso-list:Ignore'>1.<span style='font:7.0pt \"Times New Roman\"'>&nbsp;&nbsp;&nbsp; </span></span>One</p>\
                    <p style='mso-list:l1 level1 lfo2'><span style='mso-list:Ignore'>2.<span>&nbsp;</span></span>Two</p>\
                    <p style='mso-list:l1 level2 lfo2'><span style='mso-list:Ignore'>a)<span>&nbsp;</span></span>Lettered</p>";
    assert_eq!(dump(numbered), ["ol0#1|One", "ol0#2|Two", "ol1#a)|Lettered"]);
    // A marker span outside a list paragraph is just not shown.
    assert_eq!(dump("<p>a<span style='mso-list:Ignore'>·</span>b</p>"), ["p|ab"]);
    assert_eq!(dump(&format!("<p style='mso-list:l0 level1 lfo1'><span style='mso-list:Ignore'>{}</span>x</p>", "9".repeat(5000))), ["ul0|x"]);
}

#[test]
fn blockquotes_nest() {
    let html = "<p>top</p><blockquote>one<blockquote><p>two</p></blockquote>back</blockquote>out";
    assert_eq!(dump(html), ["p|top", "p|q1|one", "p|q2|two", "p|q1|back", "p|out"]);
    assert_eq!(dump("<blockquote><ul><li>x</li></ul><hr></blockquote>"), ["ul0|q1|x", "hr|q1|"]);
    assert_eq!(dump("a</blockquote>b"), ["p|ab"]);
}

#[test]
fn preformatted_text_keeps_its_shape() {
    assert_eq!(dump("<pre>\n  fn main() {\n\tx  y\n  }\n</pre>after"), ["pre|  fn main() {\n\tx  y\n  }", "p|after"]);
    assert_eq!(dump("<pre>a\r\nb\rc</pre>"), ["pre|a\nb\nc"]);
    assert_eq!(dump("<pre>a<br>b &lt;x&gt; <b>c</b></pre>"), ["pre|a\nb <x> [B:c]"]);
    assert_eq!(dump("<pre>\n\n</pre><pre>   </pre>x"), ["p|x"]);
    // Only a line break right after the tag is dropped.
    assert_eq!(dump("<pre><code>\nx</code></pre>"), ["pre|[C:\nx]"]);
}

#[test]
fn rules_and_images() {
    assert_eq!(dump("a<hr>b"), ["p|a", "hr|", "p|b"]);
    assert_eq!(dump(r#"<img src="cid:logo@example.org" alt="Our  logo">"#), ["img(cid:logo@example.org)|Our logo"]);
    assert_eq!(dump(r#"text <img src="https://example.org/a.png"> more"#), ["p|text", "img(https://example.org/a.png)|", "p|more"]);
    assert_eq!(dump(r#"<a href="https://example.org"><img src="cid:x" alt="Shop"></a>"#), ["img(cid:x)|[:Shop→https://example.org]"]);
    // A picture inside a line of text stays with it: nothing but a line break in between.
    assert_eq!(spaces(r#"<p>before</p><p>text <img src="cid:a"> more</p>"#), [0, 1, 0, 0]);
    assert_eq!(spaces(r#"<div>x</div><div><img src="cid:a"><img src="cid:b"></div><p><img src="cid:c"></p>"#), [0, 0, 0, 1]);
}

#[test]
fn pictures_carry_the_size_they_ask_for_and_where_they_lead() {
    assert_eq!(dump(r#"<img src="cid:a" width="120" height=60>"#), ["img(cid:a 120x60)|"]);
    assert_eq!(dump(r#"<img src="cid:a" width="624px">"#), ["img(cid:a 624x)|"]);
    // A percentage, nonsense and absurd sizes ask for nothing, or for what is possible.
    assert_eq!(dump(r#"<img src="cid:a" width="100%" height="auto">"#), ["img(cid:a)|"]);
    assert_eq!(dump(r#"<img src="cid:a" width="99999999" height="1e9">"#), ["img(cid:a 8192x)|"]);
    let blocks = parse(r#"<center><a href="https://example.org/shop"><img src="https://example.org/hero.jpg" alt=""></a></center>"#);
    let Some(Block { kind: BlockKind::Image { link, .. }, align, spans, .. }) = blocks.first() else { panic!("{blocks:?}") };
    assert_eq!((link.as_deref(), *align, spans.len()), (Some("https://example.org/shop"), Align::Center, 0));
}

#[test]
fn unsafe_or_tiny_images_leave_at_most_their_alt_text() {
    assert_eq!(dump(r#"see <img src="data:image/png;base64,AAAA" alt="chart"> here"#), ["p|see chart here"]);
    assert_eq!(dump(r#"<img src="javascript:alert(1)">x<img alt="">y<img>"#), ["p|xy"]);
    assert_eq!(dump(r#"a<img src="https://example.org/t.gif" width="1" height="1" alt="t">b"#), ["p|ab"]);
    assert_eq!(dump(r#"a<img src="https://example.org/t.gif" height=0>b"#), ["p|ab"]);
}

#[test]
fn tracking_pixels_and_hidden_pictures_are_dropped() {
    for pixel in [
        r#"<img src="https://t.example/o.gif?u=42" width="1" height="1">"#,
        r#"<img src="https://t.example/o.gif" width=2 height=2 alt="">"#,
        r#"<img src="https://t.example/o.gif" style="width:1px;height:1px">"#,
        r#"<img src="https://t.example/o.gif" style="Display : None; width:1px" alt="t">"#,
        r#"<img src="https://t.example/o.gif" style="visibility:hidden">"#,
        r#"<img src="https://t.example/o.gif" hidden>"#,
        r#"<img src="https://t.example/o.gif" width="0px" height="0">"#,
        r#"<img src="https://t.example/o.gif" height="1" width="600">"#,
        r#"<div style="display:none"><img src="https://t.example/o.gif" width=600 height=400 alt="x"></div>"#,
    ] {
        assert_eq!(dump(&format!("a{pixel}b")).concat().replace("p|", ""), "ab", "{pixel}");
    }
    assert_eq!(dump("<img src=\"cid:x\" style=\"display:block\">"), ["img(cid:x)|"]);
    assert_eq!(dump("<img src=\"cid:x\" width=3 height=3>"), ["img(cid:x 3x3)|"]);
}

#[test]
fn hidden_content_is_not_shown() {
    for hidden in [
        r#"<div style="display:none">SECRET</div>"#,
        r#"<div style="display:none;max-height:0;overflow:hidden">SECRET <b>SECRET</b><p>SECRET</p></div>"#,
        r#"<span style="display:none !important; visibility:hidden; opacity:0; color:transparent; height:0; width:0">SECRET</span>"#,
        r#"<span style="font-size:0px">SECRET</span>"#,
        r#"<span style="font-size:1px;color:#ffffff;line-height:1px">SECRET</span>"#,
        r#"<font style="font-size:0">SECRET</font>"#,
        r#"<p style="visibility:hidden">SECRET</p>"#,
        r#"<div style="max-height:0px;overflow:hidden">SECRET</div>"#,
        r#"<div style="height:0;overflow:hidden">SECRET</div>"#,
        r#"<div style="mso-hide:all;display:none">SECRET</div>"#,
        r#"<div style="font-size:0;line-height:0">SECRET<b>SECRET</b><p>SECRET</p></div>"#,
        r#"<div hidden>SECRET</div>"#,
        r#"<table style="display:none"><tr><td>SECRET</td></tr></table>"#,
        r#"<table><tr style="display:none"><td>SECRET</td></tr></table>"#,
        r#"<table><tr><td style="display:none">SECRET</td></tr></table>"#,
        r#"<ul style="display:none"><li>SECRET</li></ul>"#,
        r#"<h1 style="display:none">SECRET</h1>"#,
        r#"<blockquote style="display:none">SECRET</blockquote>"#,
        r#"<a href="https://example.org" style="display:none">SECRET</a>"#,
        r#"<div style="display:none"><div>SECRET<div>SECRET</div></div>SECRET</div>"#,
        r#"<div style="font-size:16px"><span style="font-size:5%">SECRET</span></div>"#,
        r#"<div style="display:none"><img src="cid:x" alt="SECRET"><hr></div>"#,
    ] {
        let html = format!("<p>before</p>{hidden}<p>after</p>");
        let blocks = parse(&html);
        let text = all_text(&blocks);
        assert!(!text.contains("SECRET"), "{hidden}: {text}");
        assert!(text.contains("before") && text.contains("after"), "{hidden}: {text}");
        assert!(!blocks.iter().any(|b| matches!(b.kind, BlockKind::Image { .. } | BlockKind::Rule)), "{hidden}");
    }
    // What is hidden ends where its element ends, and nothing else is lost.
    assert_eq!(dump("a<span style='display:none'>x</span>b <span style='display:none'>y</span> c"), ["p|ab c"]);
    assert_eq!(dump("<table><tr><td style='display:none'>x</td><td>shown</td></tr></table>"), ["p|shown"]);
    // Text is not hidden by its colour: it is shown, and whoever paints it sees to the contrast.
    assert_eq!(dump("<span style='color:#fff;background:#fff'>visible</span>"), ["p|[#ffffff@ffffff:visible]"]);
}

#[test]
fn colours_sizes_and_alignment_are_inherited() {
    let html = "<div style='color:#333;font-size:16px;text-align:center'>a<p>b <span style='font-size:150%'>big</span></p><div style='text-align:right;color:blue'>c</div>d</div>e";
    assert_eq!(dump(html), ["p.c|[#333333^16:a]", "p.c|[#333333^16:b ][#333333^24:big]", "p.r|[#0000ff^16:c]", "p.c|[#333333^16:d]", "p|e"]);
    assert_eq!(dump("<center>a<div align=left>b</div></center><p align=right>c</p><div align=justify>d</div>"), ["p.c|a", "p|b", "p.r|c", "p|d"]);
    // Old-style attributes, and a style that says otherwise.
    assert_eq!(dump("<font color='#C00000' size='+2'>a</font><font color=navy style='color:green'>b</font>"), ["p|[#c00000^24:a][#008000:b]"]);
    assert_eq!(
        dump("<b style='font-weight:normal'>a</b><span style='font-weight:bold;font-style:italic;text-decoration:underline'>b</span>"),
        ["p|a[BIU:b]"]
    );
    assert_eq!(dump("<u style='text-decoration:none'>a</u><s>b<span style='text-decoration:none'>c</span></s>"), ["p|a[S:b]c"]);
    assert_eq!(
        dump("<small>a</small><big>b</big><mark>c</mark>x<span style='background-color:yellow'>d</span>"),
        ["p|[^11:a][^16:b][@ffff00:c]x[@ffff00:d]"]
    );
    // Sizes are clamped to what can be read.
    assert_eq!(
        dump("<span style='font-size:300px'>a</span><span style='font-size:7px'>b</span>"),
        [format!("p|[^{MAX_FONT_PX}:a][^{MIN_FONT_PX}:b]")]
    );
    // The message's own text colour, and a body background that is white anyway.
    assert_eq!(dump("<body text='#222222' bgcolor='#FFFFFF' link='#0563C1'><p>a</p></body>"), ["p|[#222222:a]"]);
    // It lasts to the end of the message, whatever follows `</body>`.
    assert_eq!(dump("<body style='color:#444'>a</body>b"), ["p|[#444444:a]", "p|[#444444:b]"]);
}

#[test]
fn a_block_with_a_background_is_a_box() {
    assert_eq!(dump("<div style='background:#f4f4f4'><p>a</p><p>b</p></div>after"), ["layout w=100%[(@f4f4f4 p|a ; p|b)]", "p|after"]);
    assert_eq!(dump("<p style='background-color:#ffc'>note</p>"), ["layout w=100%[(@ffffcc p|note)]"]);
    assert_eq!(dump("<body bgcolor='#202020' text='#eeeeee'><div>a</div></body>"), ["layout w=100%[(@202020 p|[#eeeeee:a])]"]);
    // White on what is white anyway is nothing; white on a tint is a card.
    assert_eq!(dump("<div style='background:#fff'><div style='background-color:#FEFEFE'>a</div></div>"), ["p|a"]);
    assert_eq!(
        dump("<div style='background:#eee'><div style='background:#fff'>card</div></div>"),
        ["layout w=100%[(@eeeeee layout w=100%[(@ffffff p|card)])]"]
    );
    // An empty box is nothing; a frame without a background is still a box.
    assert_eq!(dump("<div style='background:#eee'> <br> </div>x"), ["p|x"]);
    assert_eq!(dump("<div style='border:1px solid #ccc'>framed</div>"), ["layout+outline w=100%[(p|framed)]"]);
    // Quoting and lists start afresh inside a box, and go on after it.
    assert_eq!(
        dump("<blockquote>q<div style='background:#eee'>in<blockquote>deeper</blockquote></div>q</blockquote>"),
        ["p|q1|q", "layout w=100%[(@eeeeee p|in ; p|q1|deeper)]|q1", "p|q1|q"]
    );
}

#[test]
fn borders_are_a_rule_a_quote_bar_or_a_frame() {
    // Outlook separates the message it quotes with a paragraph that has a line above it.
    let outlook = "<p class=MsoNormal>Reply</p><div style='border:none;border-top:solid #E1E1E1 1.0pt;padding:3.0pt 0in 0in 0in'>\
                   <p class=MsoNormal><b>From:</b> Ada</p></div><p class=MsoNormal>Quoted</p>";
    assert_eq!(dump(outlook), ["p|Reply", "hr|", "p|[B:From:] Ada", "p|Quoted"]);
    assert_eq!(dump("<div style='border-bottom:1px solid #000'>title</div>x"), ["p|title", "hr|", "p|x"]);
    // A line down the left is how some programs quote.
    assert_eq!(
        dump("<div style='border-left:2px solid #ccc;padding-left:8px'>quoted<div>more</div></div>out"),
        ["p|q1|quoted", "p|q1|more", "p|out"]
    );
    // A blockquote is one quote, with or without its line.
    assert_eq!(dump("<blockquote style='border-left:1px solid rgb(204,204,204)'>q</blockquote>"), ["p|q1|q"]);
    assert_eq!(dump("<div style='border:0'>a</div><div style='border:none'>b</div>"), ["p|a", "p|b"]);
}

#[test]
fn data_tables() {
    let html = "<table><tr><th>Name</th><th>Qty</th></tr><tr><td>Apples</td><td> 3 </td></tr><tr><td>Pears</td><td>12</td></tr></table>";
    assert_eq!(dump(html), ["data[(th p|[B:Name])(th p|[B:Qty])][(p|Apples)(p|3)][(p|Pears)(p|12)]"]);
    // Word puts a paragraph in every cell, and a border on every cell.
    let word = "<table class=MsoTableGrid border=1 cellspacing=0 cellpadding=0><tbody><tr><td style='border:solid windowtext 1.0pt'><p class=MsoNormal>a</p></td>\
                <td style='border:solid windowtext 1.0pt;border-left:none'><p class=MsoNormal>b</p></td></tr></tbody></table>";
    assert_eq!(dump(word), ["data+border[(+trbl p|a)(+trb p|b)]"]);
    // Without the attribute, each cell has the lines it asks for, and the grid of short texts is still data.
    let lines = "<table><tr><td style='border-bottom:1px solid #ddd'>a</td><td style='border-bottom:1px solid #ddd'>1</td></tr>\
                 <tr><td style='border:none'>b</td><td style='border-left:2px solid'>2</td></tr></table>";
    assert_eq!(dump(lines), ["data[(+b p|a)(+b p|1)][(p|b)(+l p|2)]"]);
    // Unclosed rows and cells.
    assert_eq!(dump("<table><tr><td>a<td>b<tr><td>c<td>d</table>e"), ["data[(p|a)(p|b)][(p|c)(p|d)]", "p|e"]);
    // A grid of short texts is data even when nothing says so; its empty cells keep their place.
    let grid = "<table><tr><td>When:</td><td>Tuesday 3 pm</td></tr><tr><td>Where:</td><td></td></tr><tr><td>Who:</td><td>Ada</td></tr></table>";
    assert_eq!(dump(grid), ["data[(p|When:)(p|Tuesday 3 pm)][(p|Where:)()][(p|Who:)(p|Ada)]"]);
    // Cells, rows and the table carry their backgrounds, widths and spans.
    let shaded = "<table width=400 bgcolor='#eeeeee' border><tr bgcolor=navy><th colspan=2 width='50%'>T</th></tr><tr><td bgcolor='#fff'>a</td><td style='background:#ffc'>b</td></tr></table>";
    assert_eq!(dump(shaded), ["data+border w=400 bg=eeeeee[(th x2 @000080 p|[B:T])][(@ffffff p|a)(@ffffcc p|b)]"]);
    let Some(BlockKind::Table(t)) = parse(shaded).into_iter().next().map(|b| b.kind) else { panic!("no table") };
    assert_eq!((t.columns, t.rows.first().and_then(|r| r.cells.first()).and_then(|c| c.width)), (2, Some(Length::Percent(50))));
    // `<thead>` and `<caption>` say data; the caption is text above the table.
    assert_eq!(dump("<table><caption>Totals</caption><thead><tr><td>a</td></tr></thead></table>"), ["p|Totals", "data[(p|a)]"]);
}

#[test]
fn layout_tables() {
    // A table that only wraps something is that something, however deep the wrapping.
    assert_eq!(dump("<table><tr><td>only</td></tr></table>"), ["p|only"]);
    assert_eq!(
        dump("<table width='100%'><tr><td><table><tr><td><table><tr><td><p>deep</p></td></tr></table></td></tr></table></td></tr></table>"),
        ["p|deep"]
    );
    // One column of rows is those rows, each a paragraph.
    let column = "<table role=presentation><tr><td><h2>News</h2></td></tr><tr><td>Body<br>text</td></tr><tr><td>&nbsp;</td></tr><tr><td>Footer</td></tr></table>";
    assert_eq!(dump(column), ["h2|[B:News]", "p|Body\ntext", "p|Footer"]);
    assert_eq!(spaces(column), [0, 1, 1]);
    // Cells that hold more than a little text sit side by side in a layout table.
    let html = "<table><tr><td><p>One</p><p>Two</p></td><td><h3>Three</h3></td></tr></table>";
    assert_eq!(dump(html), ["layout[(p|One ; p|Two)(h3|[B:Three])]"]);
    let html = "<table><tr><td><img src='cid:logo' width=64 height=64 alt=Logo></td><td><b>Ada Lovelace</b><br>Engineer</td></tr></table>";
    assert_eq!(dump(html), ["layout[(img(cid:logo 64x64)|Logo)(p|[B:Ada Lovelace]\nEngineer)]"]);
    // Spacer cells, columns and rows make no room here.
    let spaced = "<table><tr><td width=20>&nbsp;</td><td><p>a</p><p>b</p></td><td width=20></td></tr><tr><td colspan=3 height=10></td></tr></table>";
    assert_eq!(dump(spaced), ["p|a", "p|b"]);
    let columns = "<table><tr><td><ul><li>a</li></ul></td><td>&nbsp;</td><td><ul><li>b</li></ul></td></tr><tr><td><ul><li>c</li></ul></td><td></td><td></td></tr></table>";
    assert_eq!(dump(columns), ["layout[(ul0|a)(ul0|b)][(ul0|c)()]"]);
    // A table with a table inside lays things out.
    let nested = "<table><tr><td><table><tr><th>k</th><th>v</th></tr><tr><td>a</td><td>1</td></tr></table></td><td>side</td></tr></table>";
    assert_eq!(dump(nested), ["layout[(data[(th p|[B:k])(th p|[B:v])][(p|a)(p|1)])(p|side)]"]);
    // What a wrapper asks for keeps it: a width, a background, a frame.
    assert_eq!(dump("<table width=600 align=center><tr><td>x</td></tr></table>"), ["layout.c w=600[(p|x)]"]);
    assert_eq!(
        dump("<table><tr><td bgcolor='#0066cc'><a href='https://example.org' style='color:#fff'>Shop now</a></td></tr></table>"),
        ["layout[(@0066cc p|[#ffffff:Shop now→https://example.org])]"]
    );
    assert_eq!(dump("<table style='border:1px solid #ddd'><tr><td>card</td></tr></table>"), ["layout+outline[(p|card)]"]);
    assert_eq!(dump("<table><tr><td style='border:1px solid #ddd'>card</td></tr></table>"), ["layout[(+trbl p|card)]"]);
    // A cell that is only a line is a divider, not a spacer.
    let divider = "<table width='100%'><tr><td>above</td></tr><tr><td style='border-top:1px solid #ddd;font-size:1px' height=1>&nbsp;</td></tr><tr><td>below</td></tr></table>";
    assert_eq!(dump(divider), ["layout w=100%[(p|above)][(+t )][(p|below)]"]);
    // Buttons side by side, one filled and one outlined; the gaps between them make no column.
    let buttons = "<table><tr><td bgcolor='#1a73e8'><a href='https://example.org/y' style='color:#fff'>Yes</a></td><td width=8>&nbsp;</td>\
                   <td style='border:1px solid #ccc'><a href='https://example.org/n'>No</a></td></tr></table>";
    assert_eq!(dump(buttons), ["layout[(@1a73e8 p|[#ffffff:Yes→https://example.org/y])(+trbl p|[:No→https://example.org/n])]"]);
    // A background that is the one all around is no background: wrappers in the page colour fall away.
    let wrapped = "<body bgcolor='#eeeeee'><table width='100%' bgcolor='#eeeeee'><tr><td bgcolor='#EEE'><div style='background:#eeeeee'>x</div></td></tr></table></body>";
    assert_eq!(dump(wrapped), ["layout w=100%[(@eeeeee p|x)]"]);
    assert_eq!(
        dump("<table role=presentation border=1><tr><td>a</td><td>b</td></tr><tr><td>c</td><td>d</td></tr></table>"),
        ["layout+border[(p|a)(p|b)][(p|c)(p|d)]"]
    );
}

#[test]
fn alignment_reaches_into_and_positions_tables() {
    let html = "<table width='100%'><tr><td align=center><table width=300><tr><td>a</td><td align=right><p>b</p><p>c</p></td></tr></table></td></tr></table>";
    assert_eq!(dump(html), ["layout.c w=300[(p.c|a)(p.r|b ; p.r|c)]"]);
    assert_eq!(
        dump("<table><tr align=right><td>a</td><td style='text-align:left'>b</td></tr><tr><td>c</td><td>d</td></tr></table>"),
        ["data[(p.r|a)(p|b)][(p|c)(p|d)]"]
    );
}

#[test]
fn formatting_does_not_leak_out_of_or_into_cells() {
    assert_eq!(dump("<table><tr><td><b>a</td><td>b</td></tr><tr><td>c</td><td>d</td></tr></table>e"), ["data[(p|[B:a])(p|b)][(p|c)(p|d)]", "p|e"]);
    assert_eq!(dump("<b>x<table><tr><td>cell</td></tr></table>y</b>"), ["p|[B:x]", "p|cell", "p|[B:y]"]);
    // Colour and size are inherited, as in a browser, and a link around a table still leads somewhere.
    assert_eq!(dump("<div style='color:#333;font-size:14px'><table><tr><td>cell</td></tr></table></div>"), ["p|[#333333^14:cell]"]);
    assert_eq!(
        dump("<a href='https://example.org'><table><tr><td bgcolor=navy><u>Go</u></td></tr></table></a>"),
        ["layout[(@000080 p|[U:Go→https://example.org])]"]
    );
    // A highlight is not.
    assert_eq!(dump("<span style='background:yellow'>a<table><tr><td>cell</td></tr></table></span>"), ["p|[@ffff00:a]", "p|cell"]);
}

#[test]
fn stray_table_parts() {
    assert_eq!(dump("<td>a</td><td>b</td>c"), ["layout[(p|a)(p|b)]", "p|c"]);
    assert_eq!(dump("a</td></tr></table>b"), ["p|ab"]);
    assert_eq!(dump("<tr><td>a</td></tr><p>b</p>"), ["p|a", "p|b"]);
    assert_eq!(dump("x<tr><td>a</td><td>b</td></tr><tr><td>c</td><td>d</td></tr>y"), ["p|x", "data[(p|a)(p|b)][(p|c)(p|d)]", "p|y"]);
    // Text between the cells of a real table comes before it, as in a browser.
    assert_eq!(
        dump("<table>stray<tr>text<td>a</td><td>b</td></tr><tr><td>c</td><td>d</td></tr></table>"),
        ["p|stray", "p|text", "data[(p|a)(p|b)][(p|c)(p|d)]"]
    );
    assert_eq!(dump("<table><tr><td>a</td></tr></table></td></tr>b"), ["p|a", "p|b"]);
}

#[test]
fn tables_are_capped() {
    // Rows: the last row's cell takes what follows.
    let rows: String = (0..MAX_TABLE_ROWS + 50).map(|i| format!("<tr><td>k{i}</td><td>v{i}</td></tr>")).collect();
    let blocks = parse(&format!("<table border=1>{rows}</table>after"));
    let Some(BlockKind::Table(t)) = blocks.first().map(|b| &b.kind) else { panic!("no table") };
    assert_eq!(t.rows.len(), MAX_TABLE_ROWS);
    let text = all_text(&blocks);
    assert!(text.contains(&format!("v{}", MAX_TABLE_ROWS + 49)) && blocks.last().map(Block::text).as_deref() == Some("after"));
    // Columns: the last cell takes what follows.
    let cells: String = (0..MAX_TABLE_COLUMNS + 50).map(|i| format!("<td>c{i}</td>")).collect();
    let blocks = parse(&format!("<table border=1><tr>{cells}</tr><tr><td>next</td></tr></table>"));
    let Some(BlockKind::Table(t)) = blocks.first().map(|b| &b.kind) else { panic!("no table") };
    assert_eq!((t.rows.len(), t.rows.first().map(|r| r.cells.len()), usize::from(t.columns)), (2, Some(MAX_TABLE_COLUMNS), MAX_TABLE_COLUMNS));
    assert!(all_text(&blocks).contains(&format!("c{}", MAX_TABLE_COLUMNS + 49)));
    // Spans.
    let blocks = parse("<table border=1><tr><td colspan=100000 rowspan=4000000000>a</td><td colspan=0 rowspan=-1>b</td></tr></table>");
    let Some(BlockKind::Table(t)) = blocks.first().map(|b| &b.kind) else { panic!("no table") };
    let spans: Vec<(u16, u16)> = t.rows.iter().flat_map(|r| &r.cells).map(|c| (c.colspan, c.rowspan)).collect();
    assert_eq!(spans, [(MAX_TABLE_COLUMNS as u16, MAX_ROW_SPAN), (1, 1)]);
    assert_eq!(usize::from(t.columns), MAX_TABLE_COLUMNS);
    let blocks = parse("<table border=1><tr><td colspan=99999999999999999999 rowspan=x>a</td></tr></table>");
    let Some(BlockKind::Table(t)) = blocks.first().map(|b| &b.kind) else { panic!("no table") };
    assert_eq!(t.rows.first().and_then(|r| r.cells.first()).map(|c| (c.colspan, c.rowspan)), Some((1, 1)));
}

#[test]
fn a_million_cells() {
    let html = format!("<table border=1>{}</table>", "<tr><td>x</td><td>y</td></tr>".repeat(40_000));
    let blocks = parse(&html);
    assert!(count_cells(&blocks) <= MAX_CELLS, "{}", count_cells(&blocks));
    let html = "<table><tr><td bgcolor=red>".repeat(60_000);
    let blocks = parse(&html);
    assert!(count_cells(&blocks) <= MAX_CELLS && table_depth(&blocks) <= MAX_TABLE_DEPTH);
    let html = format!("<table border=1><tr>{}", "<td>".repeat(300_000));
    assert!(count_cells(&parse(&html)) <= MAX_TABLE_COLUMNS);
    let html = "<table border=1><tr><td>a</td></tr></table>".repeat(50_000);
    assert!(count_cells(&parse(&html)) <= MAX_CELLS);
}

#[test]
fn nesting_of_tables_and_boxes_is_capped() {
    let open = "<table border=1><tr><td>x";
    let html = format!("{}{}", open.repeat(200), "</td></tr></table>".repeat(200));
    let blocks = parse(&html);
    assert_eq!(table_depth(&blocks), MAX_TABLE_DEPTH);
    // Nothing is lost: what is too deep to be a grid is still text.
    assert_eq!(all_text(&blocks).matches('x').count(), 200);
    let html = format!("{}x{}", "<div style='background:#eee'><div style='background:#ddd'>".repeat(5_000), "</div>".repeat(10_000));
    let blocks = parse(&html);
    assert_eq!(table_depth(&blocks), MAX_TABLE_DEPTH);
    assert_eq!(all_text(&blocks).trim(), "x");
    // The same background over and over is one box.
    let html = format!("{}x{}", "<div style='background:#eee'>".repeat(5_000), "</div>".repeat(5_000));
    assert_eq!(table_depth(&parse(&html)), 1);
    let mixed = "<table bgcolor=red><tr><td><div style='background:blue'><blockquote><ul><li>".repeat(3_000);
    assert!(table_depth(&parse(&mixed)) <= MAX_TABLE_DEPTH);
}

#[test]
fn scripts_styles_head_and_comments_are_dropped() {
    let html = "<html><head><title>T</title><style>p{color:red}</style><meta charset=utf-8></head>\
                <body><!-- hidden --><script>alert('x')</script><p>Shown</p><template><p>never</p></template></body></html>";
    assert_eq!(parse(html), [para("Shown")]);
    assert_eq!(dump("<p onclick=\"alert(1)\">x</p><iframe src=x>inner</iframe><object>fallback</object>"), ["p|x", "p|fallback"]);
    // A style sheet is never read: a class means nothing, whatever the sheet says about it.
    assert_eq!(dump("<style>.x{display:none;color:red;font-size:40px}</style><p class=x>shown</p>"), ["p|shown"]);
}

#[test]
fn unknown_elements_keep_their_text() {
    assert_eq!(parse("<o:p>a</o:p><custom-tag x=1>b</custom-tag><svg><text>c</text></svg>"), [para("abc")]);
    assert_eq!(parse("<textarea><b>raw</b></textarea>"), [para("<b>raw</b>")]);
}

#[test]
fn line_breaks_inside_addresses_are_removed_before_the_check() {
    assert_eq!(dump("<a href=\"https://example.org/a\n\t b\">x</a>"), ["p|[:x→https://example.org/a b]"]);
    assert_eq!(dump("<a href=\"\n  https://example.org/&#10;path  \r\n\">x</a>"), ["p|[:x→https://example.org/path]"]);
    assert_eq!(dump("<a href=\"java&#10;script:alert(1)\">x</a><a href=\"&#9;javascript:alert(1)\">y</a>"), ["p|xy"]);
    assert_eq!(dump("<a href=\"https://example.org/\u{0}\">x</a><a href=\"https://example.org/&#27;\">y</a>"), ["p|xy"]);
    assert_eq!(dump("<img src=\"cid:a\nb\">"), ["img(cid:ab)|"]);
}

#[test]
fn control_characters_are_removed() {
    assert_eq!(parse("a\u{0}b\u{7}c&#1;d\u{9b}e"), [para("abcde")]);
}

#[test]
fn broken_markup() {
    assert_eq!(parse("a < b > c <"), [para("a < b > c <")]);
    assert_eq!(dump("<p>x</p><"), ["p|x", "p|<"]);
    assert_eq!(parse("<p>ok</p><b unterminated"), [para("ok")]);
    assert_eq!(parse("<a href=\"https://example.org>x</a> and the rest"), []);
    assert_eq!(
        parse("<a href=https://example.org/unquoted>x</a>"),
        [Block::new(BlockKind::Paragraph, vec![Span::linked("x", "https://example.org/unquoted")])]
    );
    assert_eq!(parse("</p></div></table></ul></li></h1></pre></blockquote>x"), [para("x")]);
}

#[test]
fn a_gmail_style_reply() {
    let html = r#"<div dir="ltr">Thanks, see you then.<br></div><br><div class="gmail_quote"><div dir="ltr" class="gmail_attr">On Mon, Ada &lt;<a href="mailto:ada@example.org">ada@example.org</a>&gt; wrote:<br></div><blockquote class="gmail_quote" style="margin:0 0 0 .8ex;border-left:1px solid rgb(204,204,204);padding-left:1ex"><div dir="ltr">Lunch on Friday?<div><br></div><div>Ada</div></div></blockquote></div>"#;
    assert_eq!(
        dump(html),
        ["p|Thanks, see you then.", "p|On Mon, Ada <[:ada@example.org→mailto:ada@example.org]> wrote:", "p|q1|Lunch on Friday?", "p|q1|Ada"]
    );
    assert_eq!(spaces(html), [0, 1, 0, 1]);
}

#[test]
fn a_word_style_message() {
    let html = "<html xmlns:o=\"urn:schemas-microsoft-com:office:office\"><head><style><!-- p.MsoNormal {margin:0} --></style></head><body lang=EN-US>\
                <div class=WordSection1><p class=MsoNormal>Hello Bob,<o:p></o:p></p><p class=MsoNormal><o:p>&nbsp;</o:p></p>\
                <p class=MsoNormal><b>Total:</b> <span style='color:#1F497D'>42</span><o:p></o:p></p></div></body></html>";
    assert_eq!(dump(html), ["p|Hello Bob,", "p|[B:Total:] [#1f497d:42]"]);
    assert_eq!(spaces(html), [0, 1]);
}

#[test]
fn deep_nesting_is_capped_and_quick() {
    let deep = format!("{}x{}", "<blockquote>".repeat(50_000), "</blockquote>".repeat(50_000));
    let blocks = parse(&deep);
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks.first().map(|b| b.quote), Some(MAX_DEPTH as u8));
    for tag in [
        "div",
        "ul",
        "ul><li",
        "table><tr><td",
        "b",
        "pre",
        "h1",
        "a href=x",
        "span",
        "span style='color:red'",
        "div style='background:red'",
        "p style='border-left:1px solid'",
        "font size=7",
        "center",
        "td",
        "tr",
        "table",
        "div hidden",
    ] {
        let html = format!("{}x", format!("<{tag}>").repeat(40_000));
        let blocks = parse(&html);
        assert!(blocks.len() <= 2, "{tag}");
        assert!(table_depth(&blocks) <= MAX_TABLE_DEPTH, "{tag}");
    }
    let closes = "</td></tr></table></li></ul></div></span></b></p>".repeat(40_000);
    assert_eq!(parse(&format!("a{closes}b")).len(), 2);
    // Misnested formatting, over and over.
    let soup = "<b><i><span style='color:red'><a href='https://example.org'>x</b></i></span></a>".repeat(20_000);
    assert_eq!(parse(&soup).len(), 1);
}

#[test]
fn huge_input_is_truncated() {
    let html = format!("<p>start</p>{}<p>end</p>", "é ".repeat(MAX_INPUT / 3 + 10));
    let blocks = parse(&html);
    assert_eq!(blocks.first(), Some(&para("start")));
    assert!(!blocks.iter().any(|b| b.text() == "end"));
    let many = "<hr>".repeat(MAX_BLOCKS + 500);
    assert_eq!(parse(&many).len(), MAX_BLOCKS);
    // Blocks inside cells count too, and the table that holds them is not lost at the cap.
    let cells = "<tr><td><p>a</p><p>b</p><p>c</p></td><td>d</td></tr>".repeat(MAX_BLOCKS);
    let blocks = parse(&format!("<table border=1>{cells}</table>"));
    assert_eq!(blocks.len(), 1);
    assert!(all_text(&blocks).lines().count() <= MAX_BLOCKS);
}

#[test]
fn absurd_attributes_are_tamed() {
    let long = "9".repeat(100_000);
    let html = format!(
        "<table border=1 width={long} cellpadding={long}><tr><td width='{long}%' colspan={long} bgcolor='#{long}'><font size={long} color='{long}'>a</font>\
         <span style='font-size:{long}px;color:rgb({long},{long},{long})'>b</span><img src='cid:x' width={long} height=-5 alt='{long}'></td></tr></table>"
    );
    let blocks = parse(&html);
    assert_eq!(table_depth(&blocks), 1);
    let Some(BlockKind::Table(t)) = blocks.first().map(|b| &b.kind) else { panic!("no table") };
    assert_eq!(t.width, None);
    for cell in t.rows.iter().flat_map(|r| &r.cells) {
        assert!(usize::from(cell.colspan) <= MAX_TABLE_COLUMNS);
        for span in cell.blocks.iter().flat_map(|b| &b.spans) {
            assert!(span.size.is_none_or(|s| (MIN_FONT_PX..=MAX_FONT_PX).contains(&s)), "{span:?}");
        }
    }
}

#[test]
fn no_block_is_blank_and_no_span_is_empty() {
    for html in [
        "<p> <b> </b> </p>",
        "<td> </td><td>x</td>",
        "a<br> <br>",
        "<pre> \n </pre>",
        "<b>x</b> <i> </i>",
        "<table><tr><td>a </td><td></td></tr></table>",
        "<div style='background:red'> </div>",
        "<p style='mso-list:l0 level1 lfo1'><span style='mso-list:Ignore'>1.</span></p>",
    ] {
        for block in parse(html) {
            check_block(&block, html);
        }
    }
}

#[test]
fn a_font_size_of_nothing_hides_text_and_not_what_is_inside() {
    // The way newsletters close the gaps round their pictures and columns: a size of nothing on
    // the cell or the block, and every text inside with a size of its own. (Read as "hidden",
    // that showed nothing at all of such a message.)
    assert_eq!(
        dump(
            r#"<table><tr><td style="font-size:0;line-height:0"><img src="https://img.example/a.png" width="600" height="200" alt="A"></td></tr></table>"#
        ),
        ["img(https://img.example/a.png 600x200)|A"]
    );
    assert_eq!(
        dump(
            r#"<div style="font-size:1px;line-height:1px"><a href="https://shop.example/"><img src="https://img.example/b.png" width="600"></a></div>"#
        ),
        ["img(https://img.example/b.png 600x)|"]
    );
    assert_eq!(dump(r#"<span style="font-size:0"><img src="cid:x" width="20" height="20"></span>"#), ["img(cid:x 20x20)|"]);
    assert_eq!(
        dump(r#"<div style="font-size:0px">GAP <div style="font-size:14px">shown</div> GAP<p style="font-size:12pt">and this</p></div>after"#),
        ["p|[^14:shown]", "p|[^16:and this]", "p|after"]
    );
    // A size given as a share of nothing is nothing; one given in pixels, or by `<font>`, is itself.
    assert_eq!(
        dump(
            "<div style='font-size:0'><span style='font-size:150%'>NO</span><span style='font-size:2em'>NO</span><small>NO</small><font size=3>yes</font></div>"
        ),
        ["p|[^16:yes]"]
    );
    // It goes through a table, as in a browser, and ends with its element.
    assert_eq!(
        dump("<div style='font-size:0'><table><tr><td>NO</td><td style='font-size:13px'>yes</td></tr></table></div><p>after</p>"),
        ["p|[^13:yes]", "p|after"]
    );
    // Neither a line break nor a picture's description is text to read at that size...
    assert_eq!(dump("a<span style='font-size:0'>x<br>y<br><img alt='NO'></span>b"), ["p|ab"]);
    // ...and what is really hidden inside stays hidden.
    assert_eq!(
        dump(
            "<td style='font-size:0'><div style='display:none;font-size:14px'>NO<img src='https://t.example/o.gif' width=600></div><img src='https://t.example/p.gif' width=1 height=1></td>"
        ),
        Vec::<String>::new()
    );
    // What a column builder writes: three levels of it round a picture and a paragraph.
    let built = r#"<table><tr><td style="direction:ltr;font-size:0px;padding:20px 0;text-align:center;"><div class="col" style="font-size:0px;text-align:left;display:inline-block;vertical-align:top;width:100%;"><table><tr><td style="font-size:0px;padding:10px 25px;"><img src="https://img.example/hero.jpg" width="550" height="auto" style="border:0;display:block;height:auto;width:100%;font-size:13px;"></td></tr><tr><td style="font-size:0px;padding:10px 25px;"><div style="font-family:Arial;font-size:13px;line-height:1;color:#000000;">The text of the column.</div></td></tr></table></div></td></tr></table>"#;
    assert_eq!(dump(built), ["img(https://img.example/hero.jpg 550x)|", "p|[#000000^13:The text of the column.]"]);
}

#[test]
fn a_size_of_nothing_does_not_pass_a_class_or_an_id() {
    // What a builder writes when the sizes that count are in its style sheet, by class: a size
    // of nothing on every cell, and nothing inline on the text. The style sheet is not read, so
    // read as in a browser without it, such a message showed none of its text.
    let built = r#"<style>.headline{font-size:28px}.footer div{font-size:13px}.code{font-size:32px}</style>
        <table><tr><td style="font-size:0px;padding:10px 25px;" align="center"><div><span class="headline">The headline</span></div></td></tr>
        <tr><td style="font-size:0px" align="center"><div><span class="code-box"><span class="code">482913</span></span></div></td></tr>
        <tr><td class="footer" style="font-size:0px"><div><span style="color:#0a0a0a">Need help?</span> <a class="link" href="mailto:help@shop.example">Write to us</a></div></td></tr></table>"#;
    let text = all_text(&parse(built));
    for shown in ["The headline", "482913", "Need help?", "Write to us"] {
        assert!(text.contains(shown), "{shown}: {text}");
    }
    assert!(!text.contains("font-size"), "{text}");
    // An element with a class or an id is not held to the nothing around it, and neither is
    // what is inside an element that asks for nothing and carries one itself...
    assert_eq!(dump("<div style='font-size:0'>GAP<span class='t'>shown</span>GAP<p id='p'>and this</p></div>"), ["p|shown", "p|and this"]);
    assert_eq!(dump("<div class='c' style='font-size:0'>GAP<b>shown</b><p>and this</p></div>"), ["p|[B:shown]", "p|and this"]);
    // ...but the text written in that element itself still is, and so is a share of nothing.
    assert_eq!(dump("<span class='c' style='font-size:0'>NO</span><p class='c' style='font-size:1px'>NO</p>after"), ["p|after"]);
    assert_eq!(dump("<div style='font-size:0'><span class='t' style='font-size:150%'>NO</span></div>after"), ["p|after"]);
    // Without a class or an id anywhere it is as in a browser: nothing shows.
    assert_eq!(dump("<div style='font-size:0'>NO<b>NO</b><p>NO</p><table><tr><td>NO</td></tr></table></div>after"), ["p|after"]);
    // What is really hidden stays hidden, class or no class.
    assert_eq!(dump("<div class='pre' style='display:none;font-size:0'><span class='t'>NO</span></div>after"), ["p|after"]);
}

#[test]
fn what_is_hidden_from_outlook_alone_is_shown() {
    // The button every program but Outlook gets, beside the one drawn for Outlook in a comment
    // that only Outlook reads: the comment is dropped, so the other one must stay.
    let button = "<div><!--[if mso]><v:roundrect href=\"https://shop.example/sale\" fillcolor=\"#556270\"><w:anchorlock/><center>FOR-OUTLOOK</center></v:roundrect><![endif]-->\
                  <a href=\"https://shop.example/sale\" style=\"background-color:#556270;color:#ffffff;display:inline-block;mso-hide:all;\">See the sale</a></div>";
    assert_eq!(dump(button), ["p|[#ffffff@556270:See the sale→https://shop.example/sale]"]);
    // The picture likewise: one for Outlook in its comment, one for everybody else.
    let picture = "<!--[if mso]><img src=\"https://img.example/for-outlook.png\" width=\"600\"><![endif]-->\
                   <!--[if !mso]><!--><div style=\"mso-hide:all\"><img src=\"https://img.example/hero@2x.png\" width=\"600\" alt=\"Hero\"></div><!--<![endif]-->";
    assert_eq!(dump(picture), ["img(https://img.example/hero@2x.png 600x)|Hero"]);
    // What is hidden from everybody says so in a way everybody reads.
    assert_eq!(dump("a<span style='mso-hide:all;display:none'>NO</span><span style='mso-hide:all;max-height:0'>NO</span>b"), ["p|ab"]);
}

#[test]
fn picture_addresses_as_real_mail_writes_them() {
    // Without its scheme: `https`, for pictures only.
    assert_eq!(dump(r#"<img src="//img.example/logo.png" width=100 height=50 alt="Logo">"#), ["img(https://img.example/logo.png 100x50)|Logo"]);
    assert_eq!(dump(r#"<img src="  //IMG.example:8443/a.png?x=1  ">"#), ["img(https://IMG.example:8443/a.png?x=1)|"]);
    assert_eq!(dump(r#"<a href="//shop.example/sale">link</a>"#), ["p|link"]);
    // Neither a path, nor a file server, nor nothing at all is an address without its scheme.
    for not_one in
        ["/a.png", "///a.png", "//", "// img.example/a.png", "//:80/a.png", "\\\\files\\share\\a.png", "//\\img.example/a.png", "a.png", "img/a.png"]
    {
        assert_eq!(dump(&format!(r#"<img src="{not_one}" alt="described">"#)), ["p|described"], "{not_one}");
    }
    // Character references, written every way.
    assert_eq!(
        dump(r#"<img src="https://img.example/a.png?u=1&amp;w=600&#38;h=2&#x26;sig=a%2Fb">"#),
        ["img(https://img.example/a.png?u=1&w=600&h=2&sig=a%2Fb)|"]
    );
    // The scheme in capitals, white space round the address, and no quotes at all.
    assert_eq!(dump(r#"<img src=" HTTPS://IMG.Example/A.PNG ">"#), ["img(HTTPS://IMG.Example/A.PNG)|"]);
    assert_eq!(dump("<img src=https://img.example/unquoted.png width=5 height=5>"), ["img(https://img.example/unquoted.png 5x5)|"]);
    // A line broken inside the address is mended; a space in it is the address's own (whoever
    // downloads it writes it the way an address has it).
    assert_eq!(dump("<img src=\"https://img.example/2026/\r\n\thero.png\">"), ["img(https://img.example/2026/hero.png)|"]);
    assert_eq!(dump(r#"<img src="https://img.example/spring sale.png">"#), ["img(https://img.example/spring sale.png)|"]);
    // The one to load is `src`: what `srcset` and a script's own attributes name is not looked at.
    assert_eq!(
        dump(r#"<img data-src="https://img.example/lazy.png" srcset="https://img.example/2x.png 2x" src="https://img.example/real.png">"#),
        ["img(https://img.example/real.png)|"]
    );
    assert_eq!(dump(r#"x<img srcset="https://img.example/2x.png 2x" data-src="https://img.example/lazy.png">y"#), ["p|xy"]);
}

#[test]
fn slices_of_one_picture_stay_the_rows_of_a_table() {
    // A picture cut into strips, a row each (and a row that only makes room): kept as rows, so
    // that they can be set edge to edge. Any other column of things is those things.
    let slices = "<table cellpadding=0 cellspacing=0><tr><td style='font-size:0'><img src='cid:a' width=300 height=100></td></tr>\
                  <tr><td><a href='https://shop.example/'><img src='cid:b' width=300 height=80></a></td></tr>\
                  <tr><td style='font-size:1px;line-height:1px' height=10>&nbsp;</td></tr></table>";
    assert_eq!(dump(slices), ["layout[(img(cid:a 300x100)|)][(img(cid:b 300x80)|)]"]);
    let side_by_side = "<table><tr><td><img src='cid:a'></td><td><img src='cid:b'></td></tr><tr><td colspan=2><img src='cid:c'></td></tr></table>";
    assert_eq!(dump(side_by_side), ["layout[(img(cid:a)|)(img(cid:b)|)][(x2 img(cid:c)|)]"]);
    assert_eq!(dump("<table><tr><td><img src='cid:a'></td></tr></table>"), ["img(cid:a)|"]);
    assert_eq!(dump("<table><tr><td><img src='cid:a'></td></tr><tr><td>caption</td></tr></table>"), ["img(cid:a)|", "p|caption"]);
    assert_eq!(spaces("<table><tr><td><img src='cid:a'></td></tr><tr><td>caption</td></tr></table>"), [0, 1]);
}

#[test]
fn a_picture_takes_its_size_from_its_style_where_its_attributes_give_none() {
    assert_eq!(dump(r#"<img src="cid:a" style="width:300px;height:120px">"#), ["img(cid:a 300x120)|"]);
    assert_eq!(dump(r#"<img src="cid:a" style="width:1.5in; height:36pt">"#), ["img(cid:a 144x48)|"]);
    // The attributes win, as they do in the program these messages are written for.
    assert_eq!(dump(r#"<img src="cid:a" width="600" height="280" style="width:320px;height:auto">"#), ["img(cid:a 600x280)|"]);
    assert_eq!(dump(r#"<img src="cid:a" width="600" style="height:90px">"#), ["img(cid:a 600x90)|"]);
    // A share of the room, `auto` and a limit say nothing of the size: the picture's own shape does.
    assert_eq!(dump(r#"<img src="cid:a" style="width:100%;max-width:600px;height:auto">"#), ["img(cid:a)|"]);
    assert_eq!(dump(r#"<img src="cid:a" width="100%" style="width:50em">"#), ["img(cid:a)|"]);
    // Absurd sizes are tamed like the attributes', and a pixel is still a tracker.
    assert_eq!(dump(r#"<img src="cid:a" style="width:99999999px">"#), ["img(cid:a 8192x)|"]);
    assert_eq!(dump(r#"a<img src="https://t.example/o.gif" style="width:1px!important;height:1px!important">b"#), ["p|ab"]);
}

/// What holds of every block, whatever the input.
pub(crate) fn check_block(block: &Block, context: &str) {
    assert!(block.space <= MAX_SPACE, "{context}: {block:?}");
    // Only a picture or text stands in the line of the block before.
    assert!(!(block.beside && matches!(block.kind, BlockKind::Table(_) | BlockKind::Rule)), "{context}: {block:?}");
    match &block.kind {
        BlockKind::Rule => assert!(block.spans.is_empty(), "{context}"),
        BlockKind::Image { src, link, .. } => {
            assert!(is_safe_url(src), "{context}");
            assert!(link.as_deref().is_none_or(is_safe_url), "{context}");
        }
        BlockKind::Table(table) => {
            assert!(block.spans.is_empty() && !table.rows.is_empty(), "{context}: {block:?}");
            assert!(table.rows.len() <= MAX_TABLE_ROWS && usize::from(table.columns) <= MAX_TABLE_COLUMNS, "{context}");
            for row in &table.rows {
                assert!(!row.cells.is_empty() && row.cells.len() <= MAX_TABLE_COLUMNS, "{context}: {block:?}");
                for cell in &row.cells {
                    assert!((1..=MAX_TABLE_COLUMNS as u16).contains(&cell.colspan) && (1..=MAX_ROW_SPAN).contains(&cell.rowspan), "{context}");
                    assert_eq!(cell.blocks.first().map_or(0, |b| b.space), 0, "{context}: {block:?}");
                    assert!(!cell.blocks.first().is_some_and(|b| b.beside), "{context}: {block:?}");
                    for inner in &cell.blocks {
                        check_block(inner, context);
                    }
                }
            }
        }
        _ => assert!(!block.text().trim().is_empty(), "{context}: {block:?}"),
    }
    for (i, span) in block.spans.iter().enumerate() {
        assert!(!span.text.is_empty(), "{context}: {block:?}");
        assert!(span.link.as_deref().is_none_or(is_safe_url), "{context}");
        assert!(span.size.is_none_or(|s| (MIN_FONT_PX..=MAX_FONT_PX).contains(&s)), "{context}: {span:?}");
        if let Some(next) = block.spans.get(i + 1) {
            assert!(!span.same_format(next), "{context}: unmerged spans in {block:?}");
        }
    }
}

proptest::proptest! {
    #[test]
    fn any_string_parses(s in proptest::prelude::any::<String>()) {
        let blocks = parse(&s);
        proptest::prop_assert!(!blocks.first().is_some_and(|b| b.beside));
        for block in blocks {
            check_block(&block, &s);
        }
    }

    #[test]
    fn tag_soup_parses_into_sane_blocks(s in "(<(/)?(p|b|i|a|br|hr|ul|ol|li|td|th|tr|table|pre|h2|blockquote|img|div|span|font|center|body|x)( (href|src|alt|style|width|bgcolor|align|colspan|hidden|class)=(\"[a-z:/ ;#0-9%-]{0,12}\"|[a-z:0-9]{0,6}))?>|[a-z &;<>\n]{0,6}){0,40}") {
        let blocks = parse(&s);
        proptest::prop_assert!(table_depth(&blocks) <= MAX_TABLE_DEPTH);
        proptest::prop_assert_eq!(blocks.first().map_or(0, |b| b.space), 0);
        for block in &blocks {
            check_block(block, &s);
        }
        // Plain text comes out of whatever came in.
        let _ = crate::blocks_to_text(&blocks);
    }

    #[test]
    fn styled_soup_parses_into_sane_blocks(s in "(<(div|p|span|td|table|tr|a|font|img|blockquote)( style=\"((color|background|font-size|display|margin|border|border-left|border-top|text-align|mso-list|font-weight|max-height|width|height|overflow|opacity):(#[0-9a-f]{3}|red|none|0|1px|40px|200%|hidden|center|solid|ignore|l0 level2|bold);?){0,4}\")?( (src|href)=\"(cid:x|https://e.example/|x)\")?>|</(div|p|span|td|table|tr|a|font|blockquote)>|[a-z ]{0,5}){0,60}") {
        let blocks = parse(&s);
        proptest::prop_assert!(table_depth(&blocks) <= MAX_TABLE_DEPTH);
        for block in &blocks {
            check_block(block, &s);
        }
    }
}

/// #150: what stands in one line with a picture says so, and nothing else does.
#[test]
fn a_picture_in_a_line_of_text_stands_beside_it() {
    let beside =
        |html: &str| parse(html).iter().map(|b| (b.text(), matches!(b.kind, BlockKind::Image { .. }), b.beside, b.space)).collect::<Vec<_>>();
    let pic = |beside: bool, space: u8| (String::new(), true, beside, space);
    let text = |t: &str, beside: bool, space: u8| (t.to_string(), false, beside, space);
    // A picture, then text in its line; text, then a picture in its line.
    assert_eq!(beside("<p><img src='cid:a' width=24 height=24> <span>12</span></p>"), [pic(false, 0), text("12", true, 0)]);
    assert_eq!(beside("<p>Total 7 <img src='cid:a' width=24 height=24></p>"), [text("Total 7", false, 0), pic(true, 0)]);
    assert_eq!(beside("<td align=center><img src='cid:a'>&nbsp;<b>3</b> left</td>"), [pic(false, 0), text("3 left", true, 0)]);
    // A block or a line break in between: a line of its own, and no blank line for the break.
    assert_eq!(beside("<img src='cid:a'><div>3</div>"), [pic(false, 0), text("3", false, 0)]);
    assert_eq!(beside("<img src='cid:a'><br>3"), [pic(false, 0), text("3", false, 0)]);
    assert_eq!(beside("<p>first</p><img src='cid:a'>"), [text("first", false, 0), pic(false, 1)]);
    // Pictures parted by no-break spaces are one line of pictures, not pictures with blank lines between.
    assert_eq!(
        beside(
            "<p style='margin:12px 0 0 0'><a href='https://example.org/a'><img src='cid:a' width=20 height=20></a>&nbsp;&nbsp;<a href='https://example.org/b'><img src='cid:b' width=20 height=20></a> <img src='cid:c' width=20 height=20></p>"
        ),
        [pic(false, 0), pic(true, 0), pic(true, 0)]
    );
    // A paragraph of nothing but a no-break space is still an empty line.
    assert_eq!(beside("<p>a</p><p>&nbsp;</p><p>b</p>"), [text("a", false, 0), text("b", false, 2)]);
    // A cell starts afresh: what is in the next cell is not beside the picture before.
    assert_eq!(beside("<table><tr><td><img src='cid:a'></td><td>12</td></tr></table>").len(), 1, "a table, whose cells hold their own lines");
    let table = parse("<table><tr><td><img src='cid:a'></td><td>12</td></tr></table>");
    let BlockKind::Table(t) = &table[0].kind else { panic!("{table:?}") };
    assert!(t.rows.iter().flat_map(|r| &r.cells).all(|c| c.blocks.iter().all(|b| !b.beside)), "{t:?}");
    // Plain text keeps the line: a space, not a line break.
    assert_eq!(crate::to_text("<p>Activations <img src='cid:a' alt='denied'> 3</p><p>Next</p>"), "Activations [denied] 3\n\nNext");
    assert_eq!(crate::to_text("<p><img src='cid:a'> 3</p>"), "3");
}

#[test]
fn style_width_max_width_and_auto_margins_lay_out_cards() {
    // A newsletter wrapper whose width is in its style: not a column the width of its narrowest cell.
    let news = "<table style='width:600px;margin:0 auto'><tr><td style='width:100%'><h2>Weekly digest</h2><p>Invented news for example.org readers.</p></td></tr></table>";
    assert_eq!(dump(news), ["layout.c w=600[(h2|[B:Weekly digest] ; p|Invented news for example.org readers.)]"]);
    assert_eq!(
        dump("<table style='width:80%'><tr><td><p>One</p><p>Two</p></td><td><h3>Three</h3></td></tr></table>"),
        ["layout w=80%[(p|One ; p|Two)(h3|[B:Three])]"]
    );
    // The style beats the attribute, as in the cascade.
    assert_eq!(
        dump("<table width=100 style='width:100%'><tr><td><p>One</p><p>Two</p></td><td><h3>Three</h3></td></tr></table>"),
        ["layout w=100%[(p|One ; p|Two)(h3|[B:Three])]"]
    );
    assert_eq!(
        dump("<table width=300 style='width:bogus'><tr><td><p>One</p><p>Two</p></td><td><h3>Three</h3></td></tr></table>"),
        ["layout w=300[(p|One ; p|Two)(h3|[B:Three])]"]
    );
    // Cells too.
    let cells = "<table><tr><td style='width:200px'><p>a</p><p>b</p></td><th style='width:50%'>c</th></tr></table>";
    assert_eq!(cell_widths(cells), [Some(Length::Px(200)), Some(Length::Percent(50))]);
    // A card of a div: at most so wide, centred by its automatic margins.
    let card = "<div style='max-width:600px;margin:0 auto'><p>Hello from the shop.</p></div>";
    assert_eq!(dump(card), ["layout.c w=600[(p|Hello from the shop.)]"]);
    for margins in ["margin:auto", "margin:0 auto 0 auto", "margin-left:auto;margin-right:auto", "margin:10px auto 4px"] {
        assert_eq!(dump(&format!("<div style='max-width:600px;{margins}'>x</div>")), ["layout.c w=600[(p|x)]"], "{margins}");
    }
    // Only one side automatic is not centring.
    assert_eq!(dump("<div style='max-width:600px;margin-left:auto'>x</div>"), ["layout w=600[(p|x)]"]);
    assert_eq!(dump("<div style='max-width:600px'>x</div>"), ["layout w=600[(p|x)]"]);
    // Centring with no width asked changes nothing.
    assert_eq!(dump("<div style='margin:0 auto'>x</div>"), ["p|x"]);
    // A table's max-width caps its width; zero still hides.
    assert_eq!(
        dump("<table style='max-width:500px' width='100%'><tr><td><p>One</p><p>Two</p></td><td><h3>Three</h3></td></tr></table>"),
        ["layout w=500[(p|One ; p|Two)(h3|[B:Three])]"]
    );
    assert_eq!(
        dump("<table width=300 style='max-width:500px'><tr><td><p>One</p><p>Two</p></td><td><h3>Three</h3></td></tr></table>"),
        ["layout w=300[(p|One ; p|Two)(h3|[B:Three])]"]
    );
    assert!(dump("<div style='max-width:0'>gone</div><p>seen</p>").iter().all(|b| !b.contains("gone")));
}

#[test]
fn hostile_style_widths_are_clamped_or_ignored() {
    fn walk(blocks: &[Block], widths: &mut Vec<Length>) {
        for b in blocks {
            if let BlockKind::Table(t) = &b.kind {
                widths.extend(t.width);
                for c in t.rows.iter().flat_map(|r| &r.cells) {
                    widths.extend(c.width);
                    walk(&c.blocks, widths);
                }
            }
        }
    }
    let huge = "9".repeat(300);
    for width in ["99999999999px", "1e9px", "-50px", "-1%", "0", "0px", "NaN", "inf", "12furlongs", "px", "calc(100% - 4px)", "600", &huge] {
        for html in [
            format!("<table style='width:{width};max-width:{width};margin:{width} auto'><tr><td style='width:{width}'>x</td></tr></table>"),
            format!("<div style='max-width:{width};margin:{width}'>x</div>"),
        ] {
            let mut widths = Vec::new();
            walk(&parse(&html), &mut widths);
            for w in widths {
                match w {
                    Length::Px(px) => assert!(px >= 1, "{width}"),
                    Length::Percent(p) => assert!((1..=100).contains(&p), "{width}"),
                }
            }
        }
    }
    assert_eq!(
        dump("<table style='width:99999999999px'><tr><td><p>One</p><p>Two</p></td><td><h3>Three</h3></td></tr></table>"),
        ["layout w=65535[(p|One ; p|Two)(h3|[B:Three])]"]
    );
    assert_eq!(
        dump("<table style='width:-50px'><tr><td><p>One</p><p>Two</p></td><td><h3>Three</h3></td></tr></table>"),
        ["layout[(p|One ; p|Two)(h3|[B:Three])]"]
    );
    assert_eq!(
        dump("<table style='width:250%'><tr><td><p>One</p><p>Two</p></td><td><h3>Three</h3></td></tr></table>"),
        ["layout w=100%[(p|One ; p|Two)(h3|[B:Three])]"]
    );
    let long = format!("<table style='width:{};width:300px'><tr><td><p>One</p><p>Two</p></td><td><h3>Three</h3></td></tr></table>", "a".repeat(5000));
    assert_eq!(dump(&long), ["layout[(p|One ; p|Two)(h3|[B:Three])]"]);
}

/// The widths of the cells of the first table.
fn cell_widths(html: &str) -> Vec<Option<Length>> {
    match parse(html).first().map(|b| &b.kind) {
        Some(BlockKind::Table(t)) => t.rows.iter().flat_map(|r| &r.cells).map(|c| c.width).collect(),
        _ => Vec::new(),
    }
}
