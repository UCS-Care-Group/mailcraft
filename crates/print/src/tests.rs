use std::sync::Arc;

use mailcraft_html::{Align, Block, BlockKind, Span};
use mailcraft_time::{Date, DateTime, Weekday};
use proptest::prelude::*;

use crate::calendar::{CalendarPrint, Entry, Style};
use crate::flow::{self, Run};
use crate::font::{self, FontKind};
use crate::list::{Card, Column};
use crate::memo::Memo;
use crate::*;

fn memo(body: &str) -> Memo {
    Memo {
        owner: "Avery Example".into(),
        from: "Jordan Sample <jordan@example.org>".into(),
        sent: "Tuesday, October 13, 2026 9:41 AM".into(),
        to: "Avery Example <avery@example.org>".into(),
        cc: "Team <team@example.org>".into(),
        subject: "Quarterly plan".into(),
        attachments: vec!["plan.pdf".into(), "budget.xlsx".into()],
        importance: String::new(),
        blocks: mailcraft_html::text_blocks(body),
        pictures: Vec::new(),
    }
}

fn texts(page: &Page) -> Vec<&Text> {
    page.items
        .iter()
        .filter_map(|i| match i {
            Item::Text(t) => Some(t),
            _ => None,
        })
        .collect()
}

/// The bytes are a PDF a reader can find its way around: the header, the cross-reference table
/// where `startxref` says, every object where the table says, and the trailer.
fn check_pdf(bytes: &[u8]) -> usize {
    assert!(bytes.starts_with(b"%PDF-1.7\n"));
    let text = String::from_utf8_lossy(bytes);
    assert!(text.ends_with("%%EOF\n"));
    let at = text.rfind("startxref\n").unwrap();
    let xref: usize = text[at + 10..].lines().next().unwrap().trim().parse().unwrap();
    assert!(bytes[xref..].starts_with(b"xref\n"));
    let table = String::from_utf8_lossy(&bytes[xref..]);
    let mut lines = table.lines().skip(1);
    let count: usize = lines.next().unwrap().split(' ').nth(1).unwrap().parse().unwrap();
    assert_eq!(lines.next().unwrap(), "0000000000 65535 f ");
    for n in 1..count {
        let line = lines.next().unwrap();
        assert_eq!(line.len(), 19, "{line:?}");
        let offset: usize = line[..10].parse().unwrap();
        let head = format!("{n} 0 obj\n");
        assert!(bytes[offset..].starts_with(head.as_bytes()), "object {n} isn't at {offset}");
    }
    assert!(table.contains(&format!("trailer\n<< /Size {count} /Root 1 0 R /Info 3 0 R >>")));
    // Every stream is as long as it says.
    let find = |from: usize, what: &[u8]| bytes[from..].windows(what.len()).position(|w| w == what).map(|i| i + from);
    let mut from = 0;
    while let Some(i) = find(from, b"/Length ") {
        let start = i + 8;
        let digits: String = bytes[start..].iter().take_while(|b| b.is_ascii_digit()).map(|b| *b as char).collect();
        let len: usize = digits.parse().unwrap();
        let s = find(start, b">>\nstream\n").unwrap() + 10;
        assert_eq!(&bytes[s + len..s + len + 10], b"\nendstream", "a stream's length is wrong");
        from = s + len;
    }
    text.matches("/Type /Page ").count()
}

#[test]
fn the_fonts_are_read_and_cover_latin_greek_and_cyrillic() {
    for kind in FontKind::ALL {
        let f = font::get(kind).unwrap();
        for c in "AéßœŁ€ΑβγΩЖжЯё".chars() {
            assert!(f.covers(c), "{kind:?} has no {c}");
        }
    }
    // A script it lacks is printed as the replacement character, not dropped.
    let cleaned = font::clean(FontKind::Regular, "a中b\u{0E01}c\u{7}");
    assert_eq!(cleaned.chars().count(), 5);
    assert!(cleaned.starts_with('a') && cleaned.ends_with('c'));
    assert!(cleaned.contains('\u{FFFD}') || cleaned.contains('?'));
}

#[test]
fn right_to_left_lines_are_set_from_the_right() {
    let style = flow::Style::new(FontKind::Regular, 10.5);
    // Words of another script in a right-to-left line keep their own order, against the right edge.
    let lines = flow::wrap_in(&[Run::new("abc def".to_string(), style)], 300.0, Align::Right, true);
    assert_eq!(lines.len(), 1);
    let texts: Vec<&Text> = lines[0].items.iter().filter_map(|i| if let Item::Text(t) = i { Some(t) } else { None }).collect();
    let joined: String = texts.iter().map(|t| t.text.as_str()).collect();
    assert_eq!(joined, "abc def");
    let first = texts.first().unwrap();
    assert!(first.x > 200.0, "{first:?}");
    // A block whose first letter is Hebrew is set from the right.
    let blocks = mailcraft_html::parse("<p>\u{5e9}\u{5dc}\u{5d5}\u{5dd} 12</p>");
    assert!(mailcraft_html::bidi::is_rtl(&blocks[0].text()));
}

#[test]
fn long_lines_wrap_within_the_width() {
    let style = flow::Style::new(FontKind::Regular, 10.5);
    let text = "The quick brown fox jumps over the lazy dog. ".repeat(20);
    let lines = flow::wrap(&[Run::new(text, style)], 200.0, Align::Left);
    assert!(lines.len() > 10);
    for l in &lines {
        for i in &l.items {
            if let Item::Text(t) = i {
                assert!(t.x + font::width(t.font, &t.text, t.size) <= 200.5, "{t:?}");
            }
        }
    }
    // A word longer than a line is broken at the edge.
    let long = "x".repeat(500);
    let lines = flow::wrap(&[Run::new(long.clone(), style)], 100.0, Align::Left);
    assert!(lines.len() > 5);
    let joined: String =
        lines.iter().flat_map(|l| &l.items).filter_map(|i| if let Item::Text(t) = i { Some(t.text.clone()) } else { None }).collect();
    assert_eq!(joined, long);
}

#[test]
fn memo_style_has_the_header_block_and_the_owner_on_every_page() {
    let body = (1..=300).map(|i| format!("Line {i} of the body, which goes on for a while.")).collect::<Vec<_>>().join("\n");
    let doc = memo::layout(&memo(&body), Paper::LETTER);
    assert!(doc.pages.len() >= 3, "{} pages", doc.pages.len());
    let first = doc.pages[0].text();
    for want in [
        "From:",
        "Jordan Sample <jordan@example.org>",
        "Sent:",
        "Tuesday, October 13, 2026 9:41 AM",
        "To:",
        "Cc:",
        "Subject:",
        "Quarterly plan",
        "Attachments:",
        "plan.pdf; budget.xlsx",
    ] {
        assert!(first.contains(want), "no {want:?} in\n{first}");
    }
    assert!(!first.contains("Importance:"));
    for (i, page) in doc.pages.iter().enumerate() {
        let t = texts(page);
        assert_eq!(t.first().unwrap().text, "Avery Example");
        assert_eq!(t.last().unwrap().text, (i + 1).to_string());
        // Nothing runs off the page.
        for text in t {
            assert!(text.y > 0.0 && text.y < Paper::LETTER.height - 20.0);
            assert!(text.x >= 0.0 && text.x + font::width(text.font, &text.text, text.size) <= Paper::LETTER.width - 50.0, "{text:?}");
        }
    }
    // The body follows on, line by line, in order.
    let all = doc.text();
    let a = all.find("Line 1 of").unwrap();
    let b = all.find("Line 300 of").unwrap();
    assert!(a < b);
}

#[test]
fn html_blocks_print_as_the_reading_pane_reads_them() {
    let html = "<h1>Heading</h1><p>Some <b>bold</b> and <i>italic</i> text.</p><ul><li>One</li><li>Two</li></ul>\
        <blockquote>Quoted line</blockquote><table border=1><tr><th>Name</th><th>Qty</th></tr><tr><td>Pens</td><td>seven</td></tr></table>\
        <pre>  code  here</pre><img src=\"https://tracker.example/p.png\" width=200 height=50>";
    let mut m = memo("");
    m.blocks = mailcraft_html::parse(html);
    let doc = memo::layout(&m, Paper::A4);
    let page = &doc.pages[0];
    let t = page.text();
    for want in ["Heading", "bold", "italic", "One", "Two", "Quoted line", "Name", "Qty", "Pens", "code  here", "not downloaded"] {
        assert!(t.contains(want), "no {want:?} in\n{t}");
    }
    let find = |s: &str| texts(page).into_iter().find(|t| t.text.contains(s)).unwrap().clone();
    assert_eq!(find("bold").font, FontKind::Bold);
    assert!(find("italic").italic);
    assert_eq!(find("code").font, FontKind::Mono);
    assert!(find("Heading").size > 15.0);
    // Table cells sit side by side.
    assert!((find("Pens").y - find("seven").y).abs() < 0.1 && find("seven").x > find("Pens").x);
    // Nothing from the web became a picture.
    assert!(doc.pictures.is_empty());
}

fn png_bytes(w: u32, h: u32, alpha: bool) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut e = png::Encoder::new(&mut out, w, h);
        e.set_color(if alpha { png::ColorType::Rgba } else { png::ColorType::Rgb });
        e.set_depth(png::BitDepth::Eight);
        let mut writer = e.write_header().unwrap();
        let n = (w * h) as usize * if alpha { 4 } else { 3 };
        let data: Vec<u8> = (0..n).map(|i| (i % 251) as u8).collect();
        writer.write_image_data(&data).unwrap();
    }
    out
}

#[test]
fn pictures_the_message_carries_are_printed_and_others_are_placeholders() {
    let mut m = memo("");
    m.blocks = mailcraft_html::parse("<p>Logo:</p><img src=\"cid:logo@example.org\"><img src=\"cid:missing@example.org\"><img src=\"cid:junk\">");
    m.pictures = vec![
        ("cid:logo@example.org".into(), Arc::new(png_bytes(40, 20, true))),
        ("cid:junk".into(), Arc::new(b"GIF89a not a picture we print".to_vec())),
    ];
    let doc = memo::layout(&m, Paper::LETTER);
    assert_eq!(doc.pictures.len(), 1);
    assert_eq!((doc.pictures[0].width, doc.pictures[0].height), (40, 20));
    let images = doc.pages[0].items.iter().filter(|i| matches!(i, Item::Image { .. })).count();
    assert_eq!(images, 1);
    assert_eq!(doc.pages[0].text().matches("[Picture]").count(), 2);
    let pdf = pdf::write(&doc);
    assert_eq!(check_pdf(&pdf), 1);
    assert!(String::from_utf8_lossy(&pdf).contains("/SMask"));
}

#[test]
fn a_jpeg_is_embedded_as_it_is() {
    // The frame header is all that is read: 3 components, 30 × 10.
    let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00, 0xFF, 0xC0, 0x00, 0x11, 0x08, 0x00, 0x0A, 0x00, 0x1E, 0x03];
    jpeg.extend_from_slice(&[0; 9]);
    let p = picture::decode(&jpeg).unwrap();
    assert_eq!((p.width, p.height), (30, 10));
    assert!(matches!(p.pixels, picture::Pixels::Jpeg { components: 3, .. }));
    // Too big, or cut short: not printed.
    let mut huge = jpeg.clone();
    huge[13..17].copy_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF]);
    assert!(picture::decode(&huge).is_none());
    assert!(picture::decode(&jpeg[..12]).is_none());
}

#[test]
fn the_pdf_is_well_formed_and_carries_only_the_glyphs_used() {
    let doc = memo::layout(&memo("Hello — Привет — Γειά σου"), Paper::LETTER);
    let bytes = pdf::write(&doc);
    assert_eq!(check_pdf(&bytes), doc.pages.len());
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("/Subtype /Type0") && text.contains("/Identity-H") && text.contains("/ToUnicode"));
    assert!(text.contains("+Inter-Regular") && text.contains("+Inter-SemiBold"));
    // Subsets: far smaller than the two font files.
    assert!(bytes.len() < 200_000, "{} bytes", bytes.len());
    // The same document, the same bytes.
    assert_eq!(bytes, pdf::write(&doc));
}

#[test]
fn a_subset_is_still_a_font() {
    let f = font::get(FontKind::Regular).unwrap();
    let used: std::collections::BTreeSet<u16> = "ЖéÅ".chars().map(|c| f.glyph(c)).collect();
    let sub = subset::subset(f.data, &used).unwrap();
    assert!(sub.len() < f.data.len() / 4);
    let face = ttf_parser::Face::parse(&sub, 0).unwrap();
    assert_eq!(face.number_of_glyphs(), f.glyph_count());
    struct Count(usize);
    impl ttf_parser::OutlineBuilder for Count {
        fn move_to(&mut self, _: f32, _: f32) {
            self.0 += 1;
        }
        fn line_to(&mut self, _: f32, _: f32) {}
        fn quad_to(&mut self, _: f32, _: f32, _: f32, _: f32) {}
        fn curve_to(&mut self, _: f32, _: f32, _: f32, _: f32, _: f32, _: f32) {}
        fn close(&mut self) {}
    }
    for g in &used {
        let mut c = Count(0);
        assert!(face.outline_glyph(ttf_parser::GlyphId(*g), &mut c).is_some(), "glyph {g} lost its outline");
    }
    // One that wasn't used is empty.
    let unused = f.glyph('Q');
    assert!(!used.contains(&unused));
    assert!(face.outline_glyph(ttf_parser::GlyphId(unused), &mut Count(0)).is_none());
}

#[test]
fn a_huge_message_is_capped() {
    let line = "word ".repeat(200);
    let mut m = memo("");
    m.blocks = (0..20_000).map(|_| Block::new(BlockKind::Paragraph, vec![Span::plain(line.clone())])).collect();
    m.to = "someone@example.org, ".repeat(5000);
    let doc = memo::layout(&m, Paper::LETTER);
    assert_eq!(doc.pages.len(), MAX_PAGES);
    assert!(doc.pages.last().unwrap().text().contains("The rest isn't printed"));
    // The recipients are a few lines, not pages.
    assert!(doc.pages[0].text().contains("Subject:"));
}

fn at(d: Date, h: u8, m: u8) -> DateTime {
    DateTime::new(d, h, m, 0).unwrap()
}

fn calendar(style: Style, from: Date, to: Date) -> CalendarPrint {
    let d = Date::new(2026, 10, 14).unwrap();
    CalendarPrint {
        owner: "Avery Example".into(),
        style,
        from,
        to,
        week_start: Weekday::Sun,
        entries: vec![
            Entry {
                title: "Design review".into(),
                location: "Room 4".into(),
                start: at(d, 10, 0),
                end: at(d, 11, 30),
                color: Rgb(60, 120, 200),
                ..Entry::default()
            },
            Entry { title: "Overlapping call".into(), start: at(d, 11, 0), end: at(d, 12, 0), color: Rgb(200, 80, 60), ..Entry::default() },
            Entry { title: "Early run".into(), start: at(d, 6, 0), end: at(d, 7, 0), color: Rgb(60, 160, 90), ..Entry::default() },
            Entry {
                title: "Conference".into(),
                start: at(d, 0, 0),
                end: at(d.add_days(2), 0, 0),
                all_day: true,
                color: Rgb(150, 90, 200),
                ..Entry::default()
            },
        ],
        ..CalendarPrint::default()
    }
}

#[test]
fn calendar_styles_have_a_page_per_day_week_and_month() {
    let d = Date::new(2026, 10, 14).unwrap();
    let day = calendar::layout(&calendar(Style::Day, d, d.add_days(2)), Paper::LETTER);
    assert_eq!(day.pages.len(), 3);
    let first = day.pages[0].text();
    for want in ["Wednesday, October 14, 2026", "Design review", "Room 4", "Overlapping call", "Early run", "Conference", "6:00 AM"] {
        assert!(first.contains(want), "no {want:?} in\n{first}");
    }
    // The second day still has the conference; the third doesn't.
    assert!(day.pages[1].text().contains("Conference"));
    assert!(!day.pages[2].text().contains("Conference"));
    // Overlapping items share the column.
    let boxes: Vec<(f32, f32)> = day.pages[0]
        .items
        .iter()
        .filter_map(|i| if let Item::Rect { x, w, stroke: Some(_), fill: Some(_), h, .. } = i { (*h > 20.0).then_some((*x, *w)) } else { None })
        .collect();
    assert!(boxes.windows(2).any(|p| (p[0].0 - p[1].0).abs() > 10.0), "{boxes:?}");

    let week = calendar::layout(&calendar(Style::Week, d, d), Paper::LETTER);
    assert_eq!(week.pages.len(), 1);
    assert!(week.pages[0].text().contains("October 11 – 17, 2026"));
    let work = calendar::layout(&calendar(Style::WorkWeek, d, d.add_days(7)), Paper::LETTER);
    assert_eq!(work.pages.len(), 2);
    assert!(work.pages[0].text().contains("October 12 – 16, 2026"));
    assert!(!work.pages[0].text().contains("Sun "));
    let month = calendar::layout(&calendar(Style::Month, d, d.add_days(40)), Paper::LETTER);
    assert_eq!(month.pages.len(), 2);
    let t = month.pages[0].text();
    assert!(t.contains("October 2026") && t.contains("10am Design") && t.contains("Wednesday"), "{t}");
    for doc in [&day, &week, &work, &month] {
        check_pdf(&pdf::write(doc));
    }
    // A range of years is cut to what a print covers.
    let long = calendar::layout(&calendar(Style::Day, d, d.add_days(5000)), Paper::LETTER);
    assert!(long.pages.len() <= MAX_PAGES);
}

#[test]
fn contacts_print_as_cards_and_tasks_as_a_table() {
    let cards: Vec<Card> = (0..60)
        .map(|i| Card {
            title: format!("Person {i}"),
            fields: vec![("E-mail".into(), format!("person{i}@example.org")), ("Mobile".into(), "+1 555 0100".into())],
        })
        .collect();
    let doc = list::cards("Avery Example", "Contacts", &cards, Paper::LETTER);
    assert!(doc.pages.len() >= 2);
    let t = doc.text();
    assert!(t.contains("Person 0") && t.contains("Person 59") && t.contains("person59@example.org"));
    // Two columns.
    let xs: std::collections::BTreeSet<i32> = texts(&doc.pages[0]).iter().filter(|t| t.text.starts_with("Person")).map(|t| t.x as i32).collect();
    assert_eq!(xs.len(), 2);

    let columns = vec![Column { title: "Subject".into(), weight: 3.0 }, Column { title: "Due Date".into(), weight: 1.0 }];
    let rows: Vec<Vec<String>> = (0..120).map(|i| vec![format!("Task number {i}"), "10/20/2026".into()]).collect();
    let doc = list::table("Avery Example", "Tasks", &columns, &rows, Paper::LETTER);
    assert!(doc.pages.len() >= 2);
    for page in &doc.pages {
        assert!(page.text().contains("Subject"), "the headings repeat");
    }
    check_pdf(&pdf::write(&doc));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn any_text_lays_out_and_writes(s in "\\PC{0,400}", w in 0f32..700.0) {
        let lines = flow::wrap(&[Run::new(s.clone(), flow::Style::new(FontKind::Regular, 10.0))], w, Align::Center);
        prop_assert!(!lines.is_empty());
        let mut m = memo(&s);
        m.subject = s.clone();
        m.blocks = mailcraft_html::parse(&s);
        let doc = memo::layout(&m, Paper::A4);
        let bytes = pdf::write(&doc);
        prop_assert!(bytes.starts_with(b"%PDF"));
    }

    #[test]
    fn any_bytes_as_a_picture_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..300), png in any::<bool>()) {
        let mut b = if png { b"\x89PNG\r\n\x1a\n".to_vec() } else { vec![0xFF, 0xD8] };
        b.extend(bytes);
        let _ = picture::decode(&b);
    }
}

/// `MAILCRAFT_PRINT_OUT=/some/dir cargo test -p mailcraft-print -- --ignored`: sample PDFs of
/// each kind, for looking at them in a PDF reader.
#[test]
#[ignore = "writes sample PDFs for looking at them"]
fn write_samples() {
    let Ok(dir) = std::env::var("MAILCRAFT_PRINT_OUT") else { return };
    let dir = std::path::Path::new(&dir);
    let mut m = memo("");
    m.blocks = mailcraft_html::parse(
        "<h1>Quarterly plan</h1><p>Hello <b>team</b>, here is the <i>plan</i> — Привет, Γειά σου, 你好.</p><ul><li>First</li><li>Second<ul><li>Nested</li></ul></li></ul>\
         <blockquote><p>On Monday Jordan wrote:</p><p>Can we meet?</p></blockquote><table border=1><tr><th>Item</th><th>Qty</th><th>Price</th></tr><tr><td>Pens</td><td>4</td><td>$2.00</td></tr></table>\
         <pre>fn main() {\n    println!(\"hi\");\n}</pre><p><img src=\"cid:logo\"></p><img src=\"https://tracker.example/p.png\" width=300 height=80>",
    );
    m.pictures = vec![("cid:logo".into(), Arc::new(png_bytes(120, 60, false)))];
    std::fs::write(dir.join("memo.pdf"), pdf::write(&memo::layout(&m, Paper::LETTER))).unwrap();
    let d = Date::new(2026, 10, 14).unwrap();
    for (name, style) in [("day", Style::Day), ("week", Style::Week), ("month", Style::Month)] {
        std::fs::write(dir.join(format!("{name}.pdf")), pdf::write(&calendar::layout(&calendar(style, d, d), Paper::LETTER))).unwrap();
    }
}
