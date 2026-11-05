// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use mailcraft_model::rich::{Align, Font, Format, ListKind, MAX_LEVEL, MAX_SIZE, MIN_SIZE, Paragraph, RichText, Run, Style, safe_color, safe_link};
use mailcraft_model::signature::{Layout, MAX_WIDTH, Signature};
use proptest::prelude::*;

use super::*;

/// Every tag in `html` is one the set writes, with only the attributes it writes, and no `<`
/// stands alone in text. The whole check of "nothing outside the set".
fn assert_in_set(html: &str) {
    let mut rest = html;
    while let Some(at) = rest.find('<') {
        let after = &rest[at..];
        let end = after.find('>').unwrap_or_else(|| panic!("an unclosed tag in {html:?}"));
        let tag = &after[..=end];
        let ok = matches!(
            tag,
            "<div>"
                | "<div style=\"text-align:center\">"
                | "<div style=\"text-align:right\">"
                | "</div>"
                | "<br>"
                | "<li>"
                | "</li>"
                | "</ul>"
                | "</ol>"
                | "<b>"
                | "</b>"
                | "<i>"
                | "</i>"
                | "<u>"
                | "</u>"
                | "</span>"
                | "</a>"
        ) || tag == "<ul style=\"margin-top:0;margin-bottom:0\">"
            || tag == "<ol style=\"margin-top:0;margin-bottom:0\">"
            || tag
                .strip_prefix("<span style=\"font-size:")
                .and_then(|t| t.strip_suffix("pt\">"))
                .and_then(|n| n.parse::<u8>().ok())
                .is_some_and(|n| (MIN_SIZE..=MAX_SIZE).contains(&n))
            || tag
                .strip_prefix("<a href=\"")
                .and_then(|t| t.strip_suffix("\">"))
                .is_some_and(|url| !url.contains(['"', '<', '>']) && safe_link(&mailcraft_html::unescape(url)).is_some())
            || tag.strip_prefix("<span style=\"color:").and_then(|t| t.strip_suffix("\">")).is_some_and(|c| safe_color(c).as_deref() == Some(c))
            || tag
                .strip_prefix("<span style=\"font-family:")
                .and_then(|t| t.strip_suffix("\">"))
                .is_some_and(|f| Font::ALL.iter().any(|font| font.css() == f))
            || is_cid_img(tag)
            || SIGNATURE_TABLE.contains(&tag);
        assert!(ok, "{tag:?} is outside the set in {html:?}");
        rest = &after[end + 1..];
    }
    assert!(!html.contains(['\0']));
}

/// The tags of a signature's picture-beside-text table.
const SIGNATURE_TABLE: [&str; 7] = [
    "<table cellpadding=\"0\" cellspacing=\"0\" border=\"0\">",
    "<tr>",
    "<td style=\"vertical-align:top;padding-right:12px\">",
    "<td style=\"vertical-align:top\">",
    "</td>",
    "</tr>",
    "</table>",
];

/// `<img src="cid:…" [width="n"] alt="…" style="border:0">`, nothing else in it.
fn is_cid_img(tag: &str) -> bool {
    let Some(rest) = tag.strip_prefix("<img src=\"cid:").and_then(|t| t.strip_suffix("\" style=\"border:0\">")) else { return false };
    let Some((cid, rest)) = rest.split_once('"') else { return false };
    let rest = match rest.strip_prefix(" width=\"") {
        Some(w) => match w.split_once('"') {
            Some((n, rest)) if n.parse::<u16>().is_ok_and(|n| (1..=MAX_WIDTH).contains(&n)) => rest,
            _ => return false,
        },
        None => rest,
    };
    let Some(alt) = rest.strip_prefix(" alt=\"") else { return false };
    !cid.is_empty() && !cid.contains(['<', '>', ' ']) && !alt.contains(['"', '<', '>'])
}

/// What any document read from HTML holds is within the set.
fn assert_clean(t: &RichText) {
    for p in t.paragraphs() {
        for r in &p.runs {
            assert!(!r.text.is_empty() && r.text.chars().all(|c| c == '\t' || !c.is_control()), "{r:?}");
            assert!(r.style.size.is_none_or(|n| (MIN_SIZE..=MAX_SIZE).contains(&n)), "{r:?}");
            assert!(r.style.link.as_deref().is_none_or(|l| safe_link(l).as_deref() == Some(l)), "{r:?}");
            assert!(r.style.color.as_deref().is_none_or(|c| safe_color(c).as_deref() == Some(c)), "{r:?}");
            assert!(r.picture.is_none(), "a picture came in from HTML: {r:?}");
        }
    }
}

fn styled(text: &str, style: Style) -> Run {
    Run::new(text, style)
}

fn aligned(text: &str, align: Align) -> Paragraph {
    Paragraph::new(vec![Run::plain(text)], ListKind::None).aligned(align)
}

fn aligns(t: &RichText) -> Vec<Align> {
    t.paragraphs().iter().map(|p| p.align).collect()
}

#[test]
fn the_set_is_written_as_html() {
    let t = RichText::from_paragraphs(vec![
        Paragraph::new(vec![Run::plain("Hi "), styled("Ada", Style { bold: true, ..Style::default() }), Run::plain(",")], ListKind::None),
        Paragraph::default(),
        Paragraph::new(vec![styled("one", Style { italic: true, underline: true, ..Style::default() })], ListKind::Bullet),
        Paragraph::new(vec![Run::plain("two")], ListKind::Bullet),
        Paragraph::new(vec![styled("big", Style { size: Some(18), ..Style::default() })], ListKind::Numbered),
        Paragraph::new(
            vec![styled("the <site> & \"more\"", Style { link: Some("https://example.org/?a=1&b=\"2\"".into()), ..Style::default() })],
            ListKind::None,
        ),
        aligned("in the middle", Align::Center),
    ]);
    let html = to_html(&t);
    assert_eq!(
        html,
        "<div>Hi <b>Ada</b>,</div>\r\n<div><br></div>\r\n\
         <ul style=\"margin-top:0;margin-bottom:0\"><li><i><u>one</u></i></li>\r\n<li>two</li>\r\n</ul>\
         <ol style=\"margin-top:0;margin-bottom:0\"><li><span style=\"font-size:18pt\">big</span></li>\r\n</ol>\
         <div><a href=\"https://example.org/?a=1&amp;b=&quot;2&quot;\">the &lt;site&gt; &amp; &quot;more&quot;</a></div>\r\n\
         <div style=\"text-align:center\">in the middle</div>\r\n"
    );
    assert_in_set(&html);
    assert_eq!(from_html(&html), t);
}

#[test]
fn white_space_is_kept_as_typed() {
    let t = RichText::from_plain("  two  spaces \tand a tab ");
    let html = to_html(&t);
    assert_eq!(html, "<div>&nbsp;&nbsp;two &nbsp;spaces &nbsp;&nbsp;&nbsp;&nbsp;and a tab&nbsp;</div>\r\n");
    // A tab comes back as spaces.
    assert_eq!(from_html(&html).text(), "  two  spaces     and a tab ");
}

#[test]
fn a_document_has_the_text_and_what_goes_below() {
    let t = RichText::from_plain("\n\nThanks!\n\n");
    let doc = to_html_document(&t, Some("<div id=\"x\">quoted</div>"));
    assert!(doc.starts_with("<html><head><meta http-equiv=\"Content-Type\" content=\"text/html; charset=utf-8\"></head><body>\r\n<div><br></div>"));
    assert!(doc.ends_with("<div id=\"x\">quoted</div>\r\n</body></html>\r\n"));
    // Blank lines at the edges come back too.
    let own = to_html_document(&t, None);
    assert_eq!(from_html(&own), t);
    assert_eq!(from_html(&to_html_document(&RichText::from_plain("\n\n"), None)).text(), "\n\n");
    assert_eq!(from_html(""), RichText::new());
}

#[test]
fn pasted_html_is_reduced_to_the_set() {
    let pasted = r#"<html><head><style>p{color:red}</style><script>alert(1)</script></head><body>
        <h1 style="font-family:Comic Sans">Title</h1>
        <p style="color:#f00;text-align:center;font-size:40px">Big <s>struck</s> <code>code</code> <strong>strong</strong> <em>em</em></p>
        <img src="https://tracker.example/x.png" alt="pixel"><hr>
        <ol><li>first</li><li><a href="javascript:alert(1)">bad link</a> <a href="cid:logo">cid</a> <a href="mailto:ada@example.org">ok</a></li></ol>
        <table><tr><td>cell 1</td><td><ul><li>in a cell</li></ul></td></tr></table>
        <p>line<br>break</p>
        </body></html>"#;
    let t = from_html(pasted);
    assert_clean(&t);
    assert_eq!(t.text(), "Title\n\nBig struck code strong em\n\nfirst\nbad link cid ok\n\ncell 1\nin a cell\n\nline\nbreak");
    let p = t.paragraphs();
    assert!(p[2].runs.iter().all(|r| r.style.size == Some(24)), "40px is shown at the most, 32px: 24pt");
    assert!(p[2].runs.iter().any(|r| r.style.bold && r.text == "strong"));
    assert_eq!(p[2].align, Align::Center);
    assert_eq!(p[4].list, ListKind::Numbered);
    assert_eq!(p[5].runs.iter().filter_map(|r| r.style.link.as_deref()).collect::<Vec<_>>(), ["mailto:ada@example.org"]);
    assert_eq!(p[8].list, ListKind::Bullet);
    assert_eq!(p[10].list, ListKind::None);
    assert_in_set(&to_html(&t));
}

#[test]
fn alignment_is_read_from_a_style_or_an_align_attribute_and_justify_start_and_end_reduce_to_left_left_and_right() {
    let t = from_html(
        r#"<div style="text-align:center">a</div><div align="right">b</div><div style="text-align:justify">c</div>
        <div style="text-align:start">d</div><div style="text-align:end">e</div><div align=center>f</div><div>g</div>"#,
    );
    assert_eq!(t.text(), "a\nb\nc\nd\ne\nf\ng");
    assert_eq!(aligns(&t), [Align::Center, Align::Right, Align::Left, Align::Left, Align::Right, Align::Center, Align::Left]);
    assert_in_set(&to_html(&t));
}

#[test]
fn a_center_wrapper_or_a_centred_cell_aligns_the_paragraphs_inside_it_but_not_its_list_items() {
    let t = from_html(
        r#"<center><div>a</div><ul><li>b<ul><li>c</li></ul></li></ul></center>
        <table><tr><td align="center">d</td><td align="right"><div>e</div><ol><li>f</li></ol></td></tr></table>"#,
    );
    let read: Vec<(String, ListKind, Align)> = t.paragraphs().iter().filter(|p| !p.is_empty()).map(|p| (p.text(), p.list, p.align)).collect();
    assert_eq!(
        read,
        [
            ("a".to_string(), ListKind::None, Align::Center),
            ("b".to_string(), ListKind::Bullet, Align::Left),
            ("c".to_string(), ListKind::Bullet, Align::Left),
            ("d".to_string(), ListKind::None, Align::Center),
            ("e".to_string(), ListKind::None, Align::Right),
            ("f".to_string(), ListKind::Numbered, Align::Left),
        ]
    );
    // Written again, no list item is styled.
    let html = to_html(&t);
    assert!(!html.contains("<li style"), "{html}");
    assert_in_set(&html);
}

#[test]
fn the_further_paragraphs_of_a_list_item_in_a_centred_wrapper_are_left_too() {
    // A paragraph inside an item, and the text after a list inside one, are part of that item.
    let t = from_html(
        r#"<center><ul><li>first<div>second</div></li></ul></center>
        <div align="right"><ol><li>a<ul><li>b</li></ul>c</li></ol></div>"#,
    );
    let read: Vec<(String, ListKind, Align)> = t.paragraphs().iter().filter(|p| !p.is_empty()).map(|p| (p.text(), p.list, p.align)).collect();
    assert_eq!(
        read,
        [
            ("first".to_string(), ListKind::Bullet, Align::Left),
            ("second".to_string(), ListKind::None, Align::Left),
            ("a".to_string(), ListKind::Numbered, Align::Left),
            ("b".to_string(), ListKind::Bullet, Align::Left),
            ("c".to_string(), ListKind::None, Align::Left),
        ]
    );
    // Written again, nothing is aligned.
    let html = to_html(&t);
    assert!(!html.contains("text-align"), "{html}");
    assert_in_set(&html);
}

#[test]
fn a_heading_or_preformatted_text_inside_a_list_item_in_a_centred_wrapper_is_left_too() {
    // They look like a heading and preformatted text, but they are part of the item all the same,
    // and so is a paragraph in a table inside one.
    let t = from_html(
        r#"<center><ul><li>first<pre>second</pre></li><li>third<h2>fourth</h2></li>
        <li>fifth<table><tr><td>sixth</td><td>seventh</td></tr></table></li></ul></center>"#,
    );
    let read: Vec<(String, Align)> = t.paragraphs().iter().filter(|p| !p.is_empty()).map(|p| (p.text(), p.align)).collect();
    let left = |s: &str| (s.to_string(), Align::Left);
    assert_eq!(read, [left("first"), left("second"), left("third"), left("fourth"), left("fifth"), left("sixth"), left("seventh")]);
    // Written again, nothing is aligned.
    let html = to_html(&t);
    assert!(!html.contains("text-align"), "{html}");
    assert_in_set(&html);
}

#[test]
fn an_empty_paragraph_is_written_bare_whatever_its_alignment_so_a_blank_lines_alignment_is_not_kept_by_a_draft() {
    let t = RichText::from_paragraphs(vec![
        aligned("", Align::Center),
        aligned("Avery", Align::Center),
        aligned("", Align::Right),
        aligned("Lumen", Align::Right),
        aligned("", Align::Center),
    ]);
    let html = to_html(&t);
    assert_eq!(
        html,
        "<div><br></div>\r\n<div style=\"text-align:center\">Avery</div>\r\n<div><br></div>\r\n<div style=\"text-align:right\">Lumen</div>\r\n<div><br></div>\r\n"
    );
    // The blank lines come back, left; the lines with text as they were.
    let back = from_html(&to_html_document(&t, None));
    assert_eq!(back.text(), "\nAvery\n\nLumen\n");
    assert_eq!(aligns(&back), [Align::Left, Align::Center, Align::Left, Align::Right, Align::Left]);
}

#[test]
fn sizes_survive_the_trip_in_points() {
    for pt in MIN_SIZE..=24 {
        let mut t = RichText::from_plain("x");
        t.format(0, 1, &Format::Size(Some(pt))).unwrap();
        assert_eq!(from_html(&to_html(&t)), t, "{pt}");
    }
    assert_eq!(px_to_points(10), MIN_SIZE);
    assert_eq!(px_to_points(255), MAX_SIZE);
}

#[test]
fn nested_lists_are_lists_in_items_and_come_back_at_their_levels() {
    let item = |text: &str, list, level| Paragraph::new(vec![Run::plain(text)], list).at_level(level);
    let t = RichText::from_paragraphs(vec![
        item("a", ListKind::Numbered, 0),
        item("a1", ListKind::Bullet, 1),
        item("deep", ListKind::Bullet, 3),
        item("b", ListKind::Numbered, 0),
        item("end", ListKind::None, 0),
    ]);
    let html = to_html(&t);
    let ol = "<ol style=\"margin-top:0;margin-bottom:0\">";
    let ul = "<ul style=\"margin-top:0;margin-bottom:0\">";
    assert_eq!(html, format!("{ol}<li>a\r\n{ul}<li>a1\r\n{ul}{ul}<li>deep</li>\r\n</ul></ul></li></ul></li><li>b</li>\r\n</ol><div>end</div>\r\n"));
    assert_in_set(&html);
    assert_eq!(from_html(&to_html_document(&t, None)), t);
    // What Outlook, Word and the web's editors paste: lists in items, a level per list, as deep
    // as the model goes.
    let pasted = from_html(
        "<ul><li>one<ul><li>two<ol><li>three<ul><ul><ul><ul><ul><ul><ul><li>far</li></ul></ul></ul></ul></ul></ul></ul></li></ol></li></ul></li></ul>",
    );
    let levels: Vec<(ListKind, u8)> = pasted.paragraphs().iter().map(|p| (p.list, p.level)).collect();
    assert_eq!(levels, [(ListKind::Bullet, 0), (ListKind::Bullet, 1), (ListKind::Numbered, 2), (ListKind::Bullet, MAX_LEVEL)]);
}

fn arb_style() -> impl Strategy<Value = Style> {
    (
        any::<bool>(),
        any::<bool>(),
        any::<bool>(),
        proptest::option::of(any::<u8>()),
        proptest::option::of(prop_oneof![
            Just("https://example.org/a?b=c&d=\"e\"".to_string()),
            Just("mailto:ada@example.org".to_string()),
            Just("javascript:alert(1)".to_string()),
            "\\PC{0,20}",
        ]),
    )
        .prop_map(|(bold, italic, underline, size, link)| Style { bold, italic, underline, size, link, ..Style::default() })
        .prop_flat_map(|s| {
            (
                Just(s),
                proptest::option::of(prop_oneof![Just("#1f6feb".to_string()), Just("red".to_string()), "\\PC{0,8}"]),
                proptest::option::of(0usize..5),
            )
                .prop_map(|(s, color, font)| Style { color, font: font.and_then(|f| Font::ALL.get(f).copied()), ..s })
        })
}

fn arb_text() -> impl Strategy<Value = RichText> {
    proptest::collection::vec((proptest::collection::vec(("\\PC{0,12}", arb_style()), 0..4), 0u8..3, any::<u8>(), 0usize..3), 1..8).prop_map(|ps| {
        RichText::from_paragraphs(
            ps.into_iter()
                .map(|(runs, list, level, align)| {
                    Paragraph::new(
                        runs.into_iter().map(|(t, s)| Run::new(t, s)).collect(),
                        [ListKind::None, ListKind::Bullet, ListKind::Numbered][list as usize],
                    )
                    .at_level(level)
                    .aligned([Align::Left, Align::Center, Align::Right][align])
                })
                .collect(),
        )
    })
}

/// Documents the HTML form can carry exactly: words separated by one space, sizes up to 24pt,
/// list items with text, and paragraphs with text aligned any way (a blank line is left).
fn arb_exact() -> impl Strategy<Value = RichText> {
    let style = (
        any::<bool>(),
        any::<bool>(),
        any::<bool>(),
        proptest::option::of(MIN_SIZE..=24u8),
        proptest::option::of(Just("https://example.org/x".to_string())),
    )
        .prop_map(|(bold, italic, underline, size, link)| Style { bold, italic, underline, size, link, ..Style::default() })
        .prop_flat_map(|s| (Just(s), proptest::option::of(Just("#1f6feb".to_string()))).prop_map(|(s, color)| Style { color, ..s }));
    let paragraph = (proptest::collection::vec(("[a-zA-Z0-9&<>\"'é]{1,6}", style), 0..4), 0u8..3, 0..=MAX_LEVEL, 0usize..3).prop_map(
        |(runs, list, level, align)| {
            let runs: Vec<Run> = runs.into_iter().enumerate().map(|(i, (t, s))| Run::new(if i == 0 { t } else { format!(" {t}") }, s)).collect();
            let list = if runs.is_empty() { ListKind::None } else { [ListKind::None, ListKind::Bullet, ListKind::Numbered][list as usize] };
            let align = if runs.is_empty() { Align::Left } else { [Align::Left, Align::Center, Align::Right][align] };
            Paragraph::new(runs, list).at_level(level).aligned(align)
        },
    );
    proptest::collection::vec(paragraph, 1..8).prop_map(|ps| RichText::from_paragraphs(at_most_three_blank_lines(ps)))
}

/// `crates/html` keeps at most 3 blank lines between blocks (`MAX_SPACE`, a bound against
/// hostile mail), so a longer run between two lines of text comes back as 3.
// ponytail: 4+ blank lines inside a draft reopen as 3; carry the count in the HTML if anyone minds.
fn at_most_three_blank_lines(ps: Vec<Paragraph>) -> Vec<Paragraph> {
    let first = ps.iter().position(|p| !p.runs.is_empty());
    let last = ps.iter().rposition(|p| !p.runs.is_empty());
    let mut blank = 0;
    let mut out = Vec::new();
    for (i, p) in ps.into_iter().enumerate() {
        let inside = first.is_some_and(|f| i > f) && last.is_some_and(|l| i < l);
        blank = if p.runs.is_empty() { blank + 1 } else { 0 };
        if !(inside && blank > 3) {
            out.push(p);
        }
    }
    out
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(300))]

    /// Whatever is pasted, what comes of it is within the set, and so is its HTML.
    #[test]
    fn pasted_html_never_escapes_the_set(html in "(<(/?)(b|i|u|s|a|p|div|ul|ol|li|table|tr|td|span|font|img|script|style|br|h1|pre|blockquote)( href=\"(javascript:x|https://example.org|cid:y|mailto:a@b.example|data:,x)\"| style=\"(font-size:[0-9]{1,3}(px|pt)|color:red|margin:0)\"| src=\"https://example.org/p.png\")?>|[a-z &;<>\"\u{a0}\\x00\\r\\n\\t]{0,8}|&nbsp;|&#0;|&lt;script&gt;){0,40}") {
        let t = from_html(&html);
        assert_clean(&t);
        assert_in_set(&to_html(&t));
        assert_in_set(&to_html_document(&t, None).replace("<html><head><meta http-equiv=\"Content-Type\" content=\"text/html; charset=utf-8\"></head><body>", "").replace("</body></html>", ""));
    }

    /// Bytes that aren't HTML at all, too.
    #[test]
    fn any_text_pasted_as_html_stays_in_the_set(html in "\\PC{0,200}") {
        let t = from_html(&html);
        assert_clean(&t);
        assert_in_set(&to_html(&t));
    }

    /// Whatever the owner wrote, the HTML that goes out holds only the set.
    #[test]
    fn any_text_is_written_within_the_set(t in arb_text()) {
        assert_in_set(&to_html(&t));
    }

    /// What is written comes back as it was.
    #[test]
    fn the_set_survives_the_trip(t in arb_exact()) {
        prop_assert_eq!(from_html(&to_html_document(&t, None)), t);
    }

    /// However hostile the text and the subject, a message written from them has no header
    /// line that isn't a header, and no NUL or bare CR or LF anywhere in its header.
    #[test]
    fn a_message_from_any_text_keeps_its_headers(t in arb_text(), subject in "\\PC{0,40}|[\\r\\n\\x00a-z:]{0,20}") {
        let m = mailcraft_model::Message {
            subject,
            from: mailcraft_model::Address::new("Ada", "ada@example.org"),
            to: vec![mailcraft_model::Address::new("Bob\r\nBcc: x@example.org", "bob@example.org")],
            body: mailcraft_model::Body { text: t.to_plain(), html: Some(to_html_document(&t, None)) },
            ..Default::default()
        };
        let bytes = crate::write(&m, &crate::WriteOptions::default());
        let end = bytes.windows(4).position(|w| w == b"\r\n\r\n").unwrap_or(bytes.len());
        let head = &bytes[..end];
        prop_assert!(!head.contains(&0));
        for (i, line) in head.split(|b| *b == b'\n').enumerate() {
            prop_assert!(line.ends_with(b"\r") || i == head.split(|b| *b == b'\n').count() - 1, "a bare LF");
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            prop_assert!(!line.contains(&b'\r'), "a bare CR");
            let folded = line.first().is_some_and(|b| *b == b' ' || *b == b'\t');
            let field = line.iter().position(|b| *b == b':').is_some_and(|c| c > 0 && line[..c].iter().all(|b| b.is_ascii_graphic()));
            prop_assert!(folded || field, "not a header line: {:?}", String::from_utf8_lossy(line));
            let name = String::from_utf8_lossy(line).to_ascii_lowercase();
            prop_assert!(!name.starts_with("bcc:"), "an injected field");
        }
        // And the body comes back as it was sent.
        let back = crate::parse(&bytes);
        prop_assert_eq!(back.body.html.as_deref().map(from_html), Some(from_html(&to_html_document(&t, None))));
    }
}

/// A PNG's signature and a few bytes.
fn png() -> Vec<u8> {
    b"\x89PNG\r\n\x1a\nIHDR".to_vec()
}

fn logo_signature(layout: Layout) -> Signature {
    let mut sig = Signature::plain_text("Work", "Avery Quinn\nLumen Studio");
    let mut body = sig.body().clone();
    body.format(0, 11, &Format::Color(Some("#1f6feb".into()))).unwrap();
    body.format(0, 11, &Format::Font(Some(Font::Georgia))).unwrap();
    body.set_align(12, 12, Align::Right).unwrap();
    sig.set_body(body).unwrap();
    let id = sig.add_picture("lumen-logo.png", png(), Some("Lumen \"Studio\" <logo>"), Some(0)).unwrap();
    sig.set_picture(id, Some(Some(120)), None).unwrap();
    sig.layout = layout;
    sig
}

#[test]
fn a_signature_is_written_with_its_colours_fonts_and_pictures() {
    let cid = |p: &mailcraft_model::signature::Picture| format!("sig-{}@mailcraft.invalid", p.id);
    let inline = signature_html(&logo_signature(Layout::Inline), &cid);
    assert!(
        inline.starts_with(
            "<div><img src=\"cid:sig-1@mailcraft.invalid\" width=\"120\" alt=\"Lumen &quot;Studio&quot; &lt;logo&gt;\" style=\"border:0\">"
        ),
        "{inline}"
    );
    assert!(inline.contains("<span style=\"color:#1f6feb\"><span style=\"font-family:Georgia, serif\">Avery Quinn</span></span>"), "{inline}");
    assert!(inline.contains("<div style=\"text-align:right\">Lumen Studio</div>\r\n"), "{inline}");
    assert_in_set(&inline);
    let left = signature_html(&logo_signature(Layout::PictureLeft), &cid);
    assert!(
        left.starts_with(
            "<table cellpadding=\"0\" cellspacing=\"0\" border=\"0\"><tr><td style=\"vertical-align:top;padding-right:12px\"><img src=\"cid:sig-1@"
        ),
        "{left}"
    );
    assert_eq!(left.matches("<img").count(), 1);
    // A line aligned right is so inside the text's cell.
    assert!(left.contains("<div style=\"text-align:right\">Lumen Studio</div>\r\n</td>"), "{left}");
    assert_in_set(&left);
    let above = signature_html(&logo_signature(Layout::PictureAbove), &cid);
    assert!(above.starts_with("<div><img") && !above.contains("<table"), "{above}");
    assert_in_set(&above);
    // What a reader gets back is the words in their colour, and no picture.
    let back = from_html(&left);
    assert_eq!(back.text(), "Avery Quinn\nLumen Studio");
    assert_eq!(back.style_at(3).color.as_deref(), Some("#1f6feb"));
    assert_eq!(aligns(&back), [Align::Left, Align::Right]);
}

#[test]
fn a_picture_above_the_signature_is_aligned_as_its_paragraph_was() {
    let cid = |p: &mailcraft_model::signature::Picture| format!("sig-{}@mailcraft.invalid", p.id);
    let mut sig = logo_signature(Layout::PictureAbove);
    let mut body = sig.body().clone();
    body.set_align(0, 0, Align::Center).unwrap();
    sig.set_body(body).unwrap();
    let above = signature_html(&sig, &cid);
    assert!(above.starts_with("<div style=\"text-align:center\"><img src=\"cid:sig-1@mailcraft.invalid\""), "{above}");
    assert!(above.contains("</div>\r\n<div style=\"text-align:center\"><span style=\"color:#1f6feb\">"), "{above}");
    assert_in_set(&above);
}

#[test]
fn a_logo_standing_twice_goes_out_beside_the_text_and_once_in_it() {
    let logo =
        Picture { id: 1, name: "logo.png".into(), mime: "image/png".into(), data: std::sync::Arc::new(png()), width: None, alt: "Lumen".into() };
    let body = RichText::from_paragraphs(vec![
        Paragraph::new(vec![Run::picture(1, Style::default()), Run::plain("Avery Quinn")], ListKind::None),
        Paragraph::new(vec![Run::plain("Lumen Studio"), Run::picture(1, Style::default())], ListKind::None),
    ]);
    let sig = Signature::assemble("Work", body, &[logo], Layout::PictureLeft);
    let left = signature_html(&sig, &|p| format!("sig-{}@mailcraft.invalid", p.id));
    // Once on the left, and once where it stood the second time; the first time it is the lead.
    assert_eq!(left.matches("<img").count(), 2, "{left}");
    assert!(left.contains("<div>Lumen Studio<img src=\"cid:sig-1@mailcraft.invalid\""), "{left}");
    assert!(left.contains("<div>Avery Quinn</div>"), "{left}");
    assert_in_set(&left);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    /// Whatever a signature holds, and however its pictures are named, its HTML is within the set.
    #[test]
    fn any_signature_is_written_within_the_set(t in arb_text(), pictures in 0usize..4, layout in 0usize..3, cid in "\\PC{0,12}", alt in "\\PC{0,12}", width in proptest::option::of(any::<u16>())) {
        let mut sig = Signature::plain_text("S", "");
        sig.set_body(t).unwrap();
        for n in 0..pictures {
            let id = sig.add_picture("p.png", png(), Some(&alt), Some(n * 3)).unwrap();
            sig.set_picture(id, Some(width), None).unwrap();
        }
        sig.layout = [Layout::Inline, Layout::PictureLeft, Layout::PictureAbove][layout];
        let html = signature_html(&sig, &|p| format!("{cid}{}", p.id));
        assert_in_set(&html);
        prop_assert!(html.matches("<img").count() <= pictures);
    }
}

/// A message's text with pictures in it, as a draft reads it back.
#[test]
fn pictures_of_a_draft_come_back_in_their_places() {
    let pic = |id: u32, width: Option<u16>, alt: &str| {
        let mut p = Picture { id, mime: "image/png".into(), width, alt: alt.into(), ..Default::default() };
        p.data = std::sync::Arc::new(png());
        p
    };
    let (a, b) = (pic(1 << 20, Some(120), "A \"quoted\" <alt> & more"), pic((1 << 20) + 1, None, ""));
    let mut text = RichText::from_plain("one two\nthree");
    text.insert_rich(4, &RichText::from_paragraphs(vec![Paragraph::new(vec![Run::picture(a.id, Style::default())], Default::default())])).unwrap();
    let end = text.len();
    text.insert_rich(end, &RichText::from_paragraphs(vec![Paragraph::new(vec![Run::picture(b.id, Style::default())], Default::default())])).unwrap();
    let html = to_html_with(&text, &|id| [&a, &b].into_iter().find(|p| p.id == id).map(|p| Img::of(p, format!("pic-{}.x@mailcraft.invalid", p.id))));
    let mut seen = Vec::new();
    let back = from_html_with_pictures(&html, &mut |img| {
        seen.push(img.clone());
        img.cid.strip_prefix("pic-")?.split('.').next()?.parse().ok()
    });
    assert_eq!(back.pictures(), vec![a.id, b.id], "{html}");
    assert_eq!(back.text(), text.text());
    assert_eq!(seen[0].width, Some(120));
    assert_eq!(seen[0].alt, "A \"quoted\" <alt> & more");
    assert_eq!(seen[1].width, None);
    // Without a claim a picture is left out, as ever.
    assert!(from_html_with_pictures(&html, &mut |_| None).pictures().is_empty());
    assert!(from_html(&html).pictures().is_empty());
}

#[test]
fn only_pictures_claimed_and_never_a_stray_object_character() {
    let html = format!(
        "<div>a{}b<img src=\"https://x.example/p.png\">c<IMG SRC='CID:q@x' width=5>d<imgx src=\"cid:q@x\"></div>",
        mailcraft_model::rich::OBJECT
    );
    let back = from_html_with_pictures(&html, &mut |img| (img.cid == "q@x").then_some(7));
    assert_eq!(back.pictures(), vec![7]);
    assert!(!back.text().replace(mailcraft_model::rich::OBJECT, "").contains(mailcraft_model::rich::OBJECT));
    assert_eq!(back.text().chars().filter(|c| *c == mailcraft_model::rich::OBJECT).count(), 1);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(300))]

    /// Whatever the HTML is, reading it for pictures does not panic and holds only claimed ones.
    #[test]
    fn any_html_reads_for_pictures(html in "(<img [a-z=\"' /:@0-9.>-]{0,30}|\\PC{0,20}){0,12}") {
        let back = from_html_with_pictures(&html, &mut |img| (img.cid.len() % 2 == 0).then_some(img.cid.len() as u32));
        prop_assert!(back.pictures().iter().all(|id| id % 2 == 0));
    }
}

#[test]
fn only_the_owners_own_draft_keeps_a_link_a_reader_would_not_follow() {
    let html = "<div>see <a href=\"ms-settings:network-vpn\">this</a> and <a href=\"java&#9;script:x\">that</a></div>";
    let links = |t: &RichText| t.paragraphs().iter().flat_map(|p| p.runs.iter()).filter_map(|r| r.style.link.clone()).collect::<Vec<_>>();
    assert!(links(&super::from_html(html)).is_empty());
    assert_eq!(links(&super::from_html_with_pictures(html, &mut |_| None)), vec!["ms-settings:network-vpn".to_string()]);
}

#[test]
fn an_object_character_written_as_an_entity_is_never_a_picture_and_a_claim_shows_one_at_most() {
    // Hostile input: OBJECT characters written as entities around a number, as if they marked a
    // picture, beside no picture claimed or beside one that is.
    let forged = |id: u32| format!("&#xFFFC;{id}&#xFFFC;&#65532;{id}&#65532;");
    let html = format!("<div>a{}b</div>", forged(7));
    assert!(from_html_with_pictures(&html, &mut |_| None).pictures().is_empty());
    let html = format!("<div>a{}<img src=\"cid:q@x\">{}b</div>", forged(7).repeat(500), forged(8));
    let back = from_html_with_pictures(&html, &mut |img| (img.cid == "q@x").then_some(7));
    assert_eq!(back.pictures(), vec![7]);
}

#[test]
fn an_object_character_written_as_an_entity_or_as_it_is_leaves_the_text_around_it_and_the_pictures_where_they_were() {
    let o = mailcraft_model::rich::OBJECT;
    let read = |html: &str| from_html_with_pictures(html, &mut |img| (img.cid == "q@x").then_some(7));
    let picture = "<img src=\"cid:q@x\">";
    for (html, text) in [
        // An even number of them, an odd one, and a pair naming the picture claimed elsewhere.
        (format!("<div>A&#65532;KEEP&#65532;B{picture}C</div>"), format!("AKEEPB{o}C")),
        (format!("<div>&#xFFFC;before{picture}after</div>"), format!("before{o}after")),
        (format!("<div>&#xFFFC;7&#xFFFC;</div><div>before{picture}after</div>"), format!("7\nbefore{o}after")),
        // Written with leading zeros, without its `;`, in capitals.
        (format!("<div>a&#x0fffc;b&#0065532 c&#XFFFC;d{picture}e</div>"), format!("ab cd{o}e")),
        // One left out by hand, or as an entity, would make an entity of what is around it.
        (format!("<div>a&#x{o}FFFC;7&#x{o}FFFC;b{picture}c</div>"), format!("a&#xFFFC;7&#xFFFC;b{o}c")),
        (format!("<div>a&#x&#xFFFC;FFFC;7&#x&#xFFFC;FFFC;b{picture}c</div>"), format!("a&#xFFFC;7&#xFFFC;b{o}c")),
    ] {
        let back = read(&html);
        assert_eq!((back.text(), back.pictures()), (text, vec![7]), "{html}");
    }
}

#[test]
fn an_img_never_closed_is_read_in_bounded_time_by_both_reads() {
    let html = "<img ".repeat(800_000);
    let started = std::time::Instant::now();
    let back = from_html_with_pictures(&html, &mut |_| Some(1));
    assert!(back.pictures().is_empty());
    assert!(started.elapsed() < std::time::Duration::from_secs(10), "{:?}", started.elapsed());
}

#[test]
fn a_picture_is_claimed_only_where_it_is_shown_and_not_when_the_end_of_what_is_read_cuts_its_tag_short() {
    let o = mailcraft_model::rich::OBJECT;
    let img = |n: usize| format!("<img src=\"cid:{n}@x\">");
    let id = |img: &ImgRef| img.cid.strip_suffix("@x").and_then(|n| n.parse().ok());
    // Pictures in a comment, in hidden text and in an attribute are not shown, so not claimed.
    let html = format!("<!--{}--><div style=\"display:none\">{}</div><div title='{}'>a{}b</div>", img(1), img(2), img(3), img(4));
    let mut asked = Vec::new();
    let back = from_html_with_pictures(&html, &mut |i| {
        asked.push(i.cid.clone());
        id(i)
    });
    assert_eq!((back.text(), back.pictures(), asked), (format!("a{o}b"), vec![4], vec!["4@x".to_string()]));
    // A line of nothing but pictures not claimed is not there, as with any picture left out, and
    // one amid a line leaves it whole.
    let read = |html: String| from_html_with_pictures(&html, &mut |_| None).text();
    assert_eq!(read(format!("<div>a</div><div>{}{}</div><div>b</div>", img(5), img(6))), "a\nb");
    assert_eq!(read(format!("<div>a{}b</div>", img(5))), "ab");
    // A picture whose tag ends where what is read ends is shown; one whose tag that end cuts
    // short is not, and is left out.
    let read_to = |short: usize| {
        let pad = "a".repeat(mailcraft_html::MAX_INPUT - "<!----><div>x".len() - img(1).len() + short);
        let back = from_html_with_pictures(&format!("<!--{pad}--><div>x{}</div>", img(1)), &mut |i| id(i));
        (back.text(), back.pictures())
    };
    assert_eq!((read_to(0), read_to(1)), ((format!("x{o}"), vec![1]), ("x".to_string(), vec![])));
}

#[test]
fn a_picture_hidden_by_its_own_tag_or_too_small_to_see_is_not_claimed_and_leaves_room_for_one_shown() {
    let o = mailcraft_model::rich::OBJECT;
    let html = "<div>a<img hidden src=\"cid:1@x\" alt=\"one\"><img style=\"display:none\" src=\"cid:2@x\"><img src=\"cid:3@x\" style=\"visibility:hidden\">\
                <img width=1 src=\"cid:4@x\"><img height=\"2px\" src=\"cid:5@x\"><img style=\"width:1px;height:1px\" src=\"cid:6@x\">b<img src=\"cid:7@x\">c</div>";
    let mut asked = Vec::new();
    // One picture may be shown yet, and the one shown takes that room.
    let back = from_html_with_pictures(html, &mut |i| {
        asked.push(i.cid.clone());
        (asked.len() == 1).then(|| i.cid.strip_suffix("@x").and_then(|n| n.parse().ok())).flatten()
    });
    assert_eq!((back.text(), back.pictures(), asked), (format!("ab{o}c"), vec![7], vec!["7@x".to_string()]));
}

#[test]
fn a_picture_a_style_sheet_hides_by_its_own_tag_or_makes_too_small_to_see_is_not_claimed_and_leaves_room_for_one_shown() {
    let o = mailcraft_model::rich::OBJECT;
    let html = "<style>img.gone{display:none} img.dot{width:1px}</style>\
                <div>a<img class=\"gone\" src=\"cid:1@x\"><img class=\"dot\" src=\"cid:2@x\">b<img src=\"cid:3@x\">c</div>";
    let mut asked = Vec::new();
    // One picture may be shown yet, and the one shown takes that room.
    let back = from_html_with_pictures(html, &mut |i| {
        asked.push(i.cid.clone());
        (asked.len() == 1).then(|| i.cid.strip_suffix("@x").and_then(|n| n.parse().ok())).flatten()
    });
    assert_eq!((back.text(), back.pictures(), asked), (format!("ab{o}c"), vec![3], vec!["3@x".to_string()]));
}

#[test]
fn a_style_sheet_rule_that_depends_on_a_pictures_place_acts_on_the_paste_as_on_the_original() {
    let o = mailcraft_model::rich::OBJECT;
    let read = |html: &str| {
        let back = from_html_with_pictures(html, &mut |img| (img.cid == "q@x").then_some(7));
        (back.text(), back.pictures())
    };
    // The text the sheet hides after the picture stays hidden, and the text the picture's place
    // keeps shown stays.
    let hides = read("<style>img + span{display:none}</style><p>a<img src=\"cid:q@x\"><span>hidden</span> b</p>");
    let shows = read("<style>span:first-child{display:none}</style><p><img src=\"cid:q@x\"><span>shown</span> b</p>");
    // So too beside a picture that is not claimed.
    let other = read("<style>img + span{display:none}</style><p>a<img src=\"cid:missing@x\"><span>hidden</span> b</p>");
    assert_eq!((hides, shows, other), ((format!("a{o} b"), vec![7]), (format!("{o}shown b"), vec![7]), ("a b".to_string(), vec![])));
}

#[test]
fn a_picture_is_claimed_where_the_style_sheet_and_its_own_style_together_show_it() {
    let o = mailcraft_model::rich::OBJECT;
    let read = |html: &str| {
        let back = from_html_with_pictures(html, &mut |img| (img.cid == "q@x").then_some(7));
        (back.text(), back.pictures())
    };
    // Every picture hidden or made one pixel wide by the sheet, and one its own style hides that
    // the sheet's `!important` shows.
    let hidden = read("<style>img { display:none }</style><div>A<img src=\"cid:q@x\">B</div>");
    let dot = read("<style>img { width:1px }</style><div>A<img src=\"cid:q@x\">B</div>");
    let shown = read("<style>img { display:block !important }</style><div>A<img style=\"display:none\" src=\"cid:q@x\">B</div>");
    assert_eq!((hidden, dot, shown), (("AB".to_string(), vec![]), ("AB".to_string(), vec![]), (format!("A{o}B"), vec![7])));
}

/// `html` read for pictures with `q@x` claimed as 7: its text, its pictures, and the pictures
/// `claim` was asked for.
fn read_q(html: &str) -> (String, Vec<u32>, Vec<String>) {
    let mut asked = Vec::new();
    let back = from_html_with_pictures(html, &mut |img| {
        asked.push(img.cid.clone());
        (img.cid == "q@x").then_some(7)
    });
    (back.text(), back.pictures(), asked)
}

#[test]
fn a_tag_naming_two_pictures_is_the_first_as_the_reading_pane_shows_it() {
    let o = mailcraft_model::rich::OBJECT;
    let mut asked = Vec::new();
    let back = from_html_with_pictures("<div>before<img src=\"cid:A\" src=\"cid:B\">after</div>", &mut |img| {
        asked.push(img.cid.clone());
        [("A", 1), ("B", 2)].iter().find(|(cid, _)| img.cid == *cid).map(|(_, id)| *id)
    });
    assert_eq!((back.text(), back.pictures(), asked), (format!("before{o}after"), vec![1], vec!["A".to_string()]));
}

#[test]
fn a_pictures_address_written_with_a_character_reference_is_read_as_the_reading_pane_reads_it() {
    let o = mailcraft_model::rich::OBJECT;
    assert_eq!(read_q("<div>a<img src=\"cid:q&#64;x\">b</div>"), (format!("a{o}b"), vec![7], vec!["q@x".to_string()]));
}

#[test]
fn a_picture_after_a_comment_holding_an_img_and_a_stray_quote_is_claimed() {
    let o = mailcraft_model::rich::OBJECT;
    assert_eq!(read_q("<div>a<!-- <img ' --><img src=\"cid:q@x\">b</div>"), (format!("a{o}b"), vec![7], vec!["q@x".to_string()]));
}

#[test]
fn a_picture_a_style_sheet_hides_by_its_address_or_its_missing_description_is_not_claimed() {
    let none = ("ab".to_string(), vec![], vec![]);
    assert_eq!(read_q("<style>img[src^=\"cid:\"]{display:none}</style><div>a<img src=\"cid:q@x\">b</div>"), none);
    assert_eq!(read_q("<style>img:not([alt]){display:none}</style><div>a<img src=\"cid:q@x\">b</div>"), none);
}

#[test]
fn a_picture_hidden_by_its_last_attribute_the_reader_keeps_is_not_claimed() {
    // 31 attributes, `hidden` the last: within the 32 the reader keeps.
    let more: String = (1..=29).map(|n| format!(" a{n}")).collect();
    assert_eq!(read_q(&format!("<div>a<img src=\"cid:q@x\"{more} hidden>b</div>")), ("ab".to_string(), vec![], vec![]));
}

#[test]
fn an_img_in_a_textarea_is_its_text_and_nothing_else() {
    let html = "<textarea><img src=\"cid:q@x\"></textarea>";
    assert_eq!(read_q(html), ("<img src=\"cid:q@x\">".to_string(), vec![], vec![]));
}

#[test]
fn a_picture_a_style_sheet_hides_by_a_class_with_an_object_character_is_not_claimed() {
    // The character written as a CSS escape in the sheet and as a reference in the class.
    let html = "<style>.x\\fffc{display:none}</style><div>a<img class=\"x&#65532;\" src=\"cid:q@x\">b</div>";
    assert_eq!(read_q(html), ("ab".to_string(), vec![], vec![]));
}

#[test]
fn a_picture_past_the_input_read_is_not_claimed_however_the_text_before_it_is_written() {
    // The comment alone, OBJECT characters written as references, is longer than what is read.
    let html = format!("<!--{}--><img src=\"cid:q@x\">", "&#65532;".repeat(262_144));
    assert_eq!(read_q(&html), (String::new(), vec![], vec![]));
}

#[test]
fn a_picture_in_text_of_no_size_is_claimed_as_the_reading_pane_shows_it() {
    // A size of nothing hides text, not pictures, from an inline style or from a sheet.
    let o = mailcraft_model::rich::OBJECT;
    let shown = (o.to_string(), vec![7], vec!["q@x".to_string()]);
    assert_eq!(read_q("<div style=\"font-size:0\"><span>a<img src=\"cid:q@x\">b</span></div>"), shown);
    assert_eq!(read_q("<style>div{font-size:0}</style><div><span>a<img src=\"cid:q@x\">b</span></div>"), shown);
}

#[test]
fn a_picture_not_taken_ends_a_table_a_cell_implied_as_the_reading_pane_ends_it() {
    let o = mailcraft_model::rich::OBJECT;
    // `parse` ends the table the cell implied at the first picture, so the `</tr>` inside the
    // hidden span closes nothing, and the second picture stays hidden.
    let html = "<td>x</td><img src=\"cid:a\"><span hidden></tr><img src=\"cid:b\"></span><img src=\"cid:c\">";
    let mut shown = Vec::new();
    collect_cids(&mailcraft_html::parse(html), &mut shown);
    assert_eq!(shown, ["a", "c"]);
    let mut asked = Vec::new();
    let back = from_html_with_pictures(html, &mut |img| {
        asked.push(img.cid.clone());
        match img.cid.as_str() {
            "b" => Some(2),
            "c" => Some(3),
            _ => None,
        }
    });
    assert_eq!((back.text(), back.pictures(), asked), (format!("x\n{o}"), vec![3], shown));
}

/// The `cid:` addresses of the pictures among `blocks`, table cells walked, in order.
fn collect_cids(blocks: &[mailcraft_html::Block], out: &mut Vec<String>) {
    for b in blocks {
        match &b.kind {
            mailcraft_html::BlockKind::Image { src, .. } => out.extend(src.strip_prefix("cid:").map(str::to_string)),
            mailcraft_html::BlockKind::Table(t) => t.rows.iter().flat_map(|r| &r.cells).for_each(|c| collect_cids(&c.blocks, out)),
            _ => {}
        }
    }
}
