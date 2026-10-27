// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use mailcraft_model::rich::{Font, Format, ListKind, MAX_LEVEL, MAX_SIZE, MIN_SIZE, Paragraph, RichText, Run, Style, safe_color, safe_link};
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
    ]);
    let html = to_html(&t);
    assert_eq!(
        html,
        "<div>Hi <b>Ada</b>,</div>\r\n<div><br></div>\r\n\
         <ul style=\"margin-top:0;margin-bottom:0\"><li><i><u>one</u></i></li>\r\n<li>two</li>\r\n</ul>\
         <ol style=\"margin-top:0;margin-bottom:0\"><li><span style=\"font-size:18pt\">big</span></li>\r\n</ol>\
         <div><a href=\"https://example.org/?a=1&amp;b=&quot;2&quot;\">the &lt;site&gt; &amp; &quot;more&quot;</a></div>\r\n"
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
    assert_eq!(p[4].list, ListKind::Numbered);
    assert_eq!(p[5].runs.iter().filter_map(|r| r.style.link.as_deref()).collect::<Vec<_>>(), ["mailto:ada@example.org"]);
    assert_eq!(p[8].list, ListKind::Bullet);
    assert_eq!(p[10].list, ListKind::None);
    assert_in_set(&to_html(&t));
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
    proptest::collection::vec((proptest::collection::vec(("\\PC{0,12}", arb_style()), 0..4), 0u8..3, any::<u8>()), 1..8).prop_map(|ps| {
        RichText::from_paragraphs(
            ps.into_iter()
                .map(|(runs, list, level)| {
                    Paragraph::new(
                        runs.into_iter().map(|(t, s)| Run::new(t, s)).collect(),
                        [ListKind::None, ListKind::Bullet, ListKind::Numbered][list as usize],
                    )
                    .at_level(level)
                })
                .collect(),
        )
    })
}

/// Documents the HTML form can carry exactly: words separated by one space, sizes up to 24pt,
/// list items with text.
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
    let paragraph = (proptest::collection::vec(("[a-zA-Z0-9&<>\"'é]{1,6}", style), 0..4), 0u8..3, 0..=MAX_LEVEL).prop_map(|(runs, list, level)| {
        let runs: Vec<Run> = runs.into_iter().enumerate().map(|(i, (t, s))| Run::new(if i == 0 { t } else { format!(" {t}") }, s)).collect();
        let list = if runs.is_empty() { ListKind::None } else { [ListKind::None, ListKind::Bullet, ListKind::Numbered][list as usize] };
        Paragraph::new(runs, list).at_level(level)
    });
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
    assert_in_set(&inline);
    let left = signature_html(&logo_signature(Layout::PictureLeft), &cid);
    assert!(
        left.starts_with(
            "<table cellpadding=\"0\" cellspacing=\"0\" border=\"0\"><tr><td style=\"vertical-align:top;padding-right:12px\"><img src=\"cid:sig-1@"
        ),
        "{left}"
    );
    assert_eq!(left.matches("<img").count(), 1);
    assert_in_set(&left);
    let above = signature_html(&logo_signature(Layout::PictureAbove), &cid);
    assert!(above.starts_with("<div><img") && !above.contains("<table"), "{above}");
    assert_in_set(&above);
    // What a reader gets back is the words in their colour, and no picture.
    let back = from_html(&left);
    assert_eq!(back.text(), "Avery Quinn\nLumen Studio");
    assert_eq!(back.style_at(3).color.as_deref(), Some("#1f6feb"));
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
