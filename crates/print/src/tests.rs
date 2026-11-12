// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;
use std::sync::Arc;

fn asset(name: &str) -> Arc<[u8]> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/fonts").join(name);
    std::fs::read(path).unwrap().into()
}

fn fonts() -> Fonts {
    Fonts {
        regular: Face::new(asset("Inter-Regular.ttf"), 0).unwrap(),
        bold: Face::new(asset("Inter-SemiBold.ttf"), 0),
        mono: Face::new(asset("JetBrainsMono-Regular.ttf"), 0),
        fallbacks: Vec::new(),
    }
}

fn doc(html: &str) -> Doc {
    Doc {
        title: "Quarterly figures".into(),
        fields: vec![
            ("From:".into(), "Ada Lovelace <ada@example.org>".into()),
            ("Sent:".into(), "Thursday, 12 November 2026 09:41".into()),
            ("To:".into(), "Charles Babbage <charles@example.org>; Team <team@example.org>".into()),
        ],
        blocks: mailcraft_html::parse(html),
        pictures: Vec::new(),
    }
}

fn dump(name: &str, bytes: &[u8]) {
    if let Some(dir) = std::env::var_os("MAILCRAFT_PRINT_DUMP") {
        std::fs::write(std::path::Path::new(&dir).join(name), bytes).unwrap();
    }
}

const SAMPLE: &str = r#"<p>Hello <b>Charles</b>,</p>
<p>Here are the figures for the quarter, as <i>promised</i>. The engine ran <u>without a fault</u> for
eleven weeks, and the <s>old</s> new gears arrived on time. See <a href="https://example.org/report">the full report</a>.</p>
<h2>Summary</h2>
<ul><li>Revenue up 12%</li><li>Costs down 3%<ul><li>mostly brass</li></ul></li></ul>
<ol><li>First</li><li>Second</li></ol>
<table border="1"><tr><th>Item</th><th>Count</th><th>Notes</th></tr>
<tr><td>Gears</td><td>1,204</td><td>From the new supplier, who delivered everything in one shipment.</td></tr>
<tr><td>Cards</td><td>88</td><td style="background:#e8f0fe">Punched</td></tr></table>
<blockquote><p>On Monday, Charles wrote:</p><p>Can you send the figures?</p></blockquote>
<pre>let x = 42;
println!("{x}");</pre>
<hr>
<p style="text-align:center;color:#a00">Centred and red</p>
<p>Averylongwordwithoutanyspacesthatmustbebrokensomewherebecauseitdoesnotfitonasinglelineofthepageatallreally.</p>"#;

#[test]
fn a_message_prints_as_a_pdf() {
    let bytes = pdf(&[doc(SAMPLE)], &fonts(), &Options::default()).unwrap();
    dump("sample.pdf", &bytes);
    assert!(bytes.starts_with(b"%PDF-"));
    assert!(bytes.len() < 200_000, "{} bytes: the fonts are subset", bytes.len());
}

#[test]
fn the_page_text_reads_in_order() {
    let pages = page_text(&[doc(SAMPLE)], &fonts(), &Options::default()).unwrap();
    let text = pages.join("\n");
    for want in [
        "Quarterly figures",
        "From:",
        "Ada Lovelace",
        "Hello Charles,",
        "Summary",
        "Revenue up 12%",
        "1,204",
        "Can you send the figures?",
        "Centred and red",
    ] {
        assert!(text.contains(want), "{want:?} missing from:\n{text}");
    }
    let order: Vec<usize> = ["Quarterly", "Hello", "Summary", "Gears", "Centred"].iter().map(|w| text.find(w).unwrap()).collect();
    assert!(order.windows(2).all(|w| w[0] < w[1]), "{text}");
    assert!(pages[0].ends_with("1 / 1"), "{}", pages[0]);
}

fn png(w: u32, h: u32, alpha: bool) -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    if alpha {
        image::RgbaImage::from_pixel(w, h, image::Rgba([200, 30, 30, 128])).write_to(&mut out, image::ImageFormat::Png).unwrap();
    } else {
        image::RgbImage::from_pixel(w, h, image::Rgb([30, 120, 200])).write_to(&mut out, image::ImageFormat::Png).unwrap();
    }
    out.into_inner()
}

fn jpeg(w: u32, h: u32) -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    image::RgbImage::from_pixel(w, h, image::Rgb([30, 160, 60])).write_to(&mut out, image::ImageFormat::Jpeg).unwrap();
    out.into_inner()
}

#[test]
fn attached_pictures_print_and_missing_ones_show_their_text() {
    let mut d = doc(
        r#"<p>Logo:</p><img src="cid:logo@example.org" width="120"><img src="cid:photo@example.org"><img src="https://example.org/x.png" alt="A remote chart">"#,
    );
    d.pictures = vec![("<logo@example.org>".into(), png(40, 20, true).into()), ("PHOTO@example.org".into(), jpeg(64, 48).into())];
    let bytes = pdf(&[d.clone()], &fonts(), &Options::default()).unwrap();
    dump("pictures.pdf", &bytes);
    let text = String::from_utf8_lossy(&bytes);
    assert_eq!(text.matches("/Subtype /Image").count(), 3, "two pictures and one soft mask");
    assert!(text.contains("/DCTDecode"), "the JPEG is passed through");
    assert!(page_text(&[d], &fonts(), &Options::default()).unwrap()[0].contains("A remote chart"));
}

#[test]
fn a_character_the_font_lacks_comes_from_a_fallback() {
    let mut f = fonts();
    let mono = f.mono.take().unwrap();
    f.fallbacks.push(mono);
    let d = doc("<p>Box: ┌─┐</p>");
    let bytes = pdf(&[d], &f, &Options::default()).unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert_eq!(text.matches("/Subtype /Type0").count(), 3, "Inter, its bold for the labels, and the fallback");
}

#[test]
fn long_messages_break_across_pages_and_each_message_starts_a_page() {
    let body: String = (1..=200).map(|n| format!("<p>Paragraph number {n} of a long message.</p>")).collect();
    let docs = [doc(&body), doc("<p>The second message.</p>")];
    let pages = page_text(&docs, &fonts(), &Options::default()).unwrap();
    assert!(pages.len() >= 4, "{} pages", pages.len());
    let last = pages.last().unwrap();
    assert!(last.contains("The second message.") && last.contains("Quarterly figures"), "{last}");
    assert!(!last.contains("Paragraph number"), "the second message starts on a page of its own");
    let first_on = |n: usize| pages.iter().position(|p| p.contains(&format!("Paragraph number {n} "))).unwrap();
    assert!(first_on(1) < first_on(200));
    assert!(pages[1].ends_with(&format!("2 / {}", pages.len())), "{}", pages[1]);
}

#[test]
fn letter_paper_is_letter_sized() {
    let bytes = pdf(&[doc("<p>x</p>")], &fonts(), &Options { paper: Paper::LETTER, ..Options::default() }).unwrap();
    assert!(String::from_utf8_lossy(&bytes).contains("/MediaBox [0 0 612 792]"));
    assert_eq!(Paper::for_region("us"), Paper::LETTER);
    assert_eq!(Paper::for_region("CH"), Paper::A4);
}

#[test]
fn hostile_layouts_print_without_hanging() {
    let mut deep = String::new();
    for _ in 0..40 {
        deep.push_str("<table><tr><td>");
    }
    deep.push_str("deep");
    let wide: String = format!("<table border=1><tr>{}</tr></table>", "<td>cell with words</td>".repeat(300));
    let tall_cell = format!("<table><tr><td>{}</td><td>x</td></tr></table>", "<p>line</p>".repeat(400));
    let huge_picture = format!(r#"<img src="cid:big" width="60000" height="60000"><p>{}</p>"#, "w".repeat(20_000));
    for html in [deep.as_str(), wide.as_str(), tall_cell.as_str(), huge_picture.as_str(), "", "<p></p>", "<ul><li></li></ul>"] {
        let mut d = doc(html);
        d.pictures = vec![("big".into(), png(3000, 2, false).into()), ("junk".into(), vec![0u8; 100].into())];
        let pages = page_text(&[d.clone()], &fonts(), &Options::default()).unwrap();
        assert!(!pages.is_empty() && pages.len() <= MAX_PAGES);
        assert!(pdf(&[d], &fonts(), &Options::default()).unwrap().starts_with(b"%PDF"));
    }
    assert_eq!(pdf(&[], &fonts(), &Options::default()), Err(Error::Empty));
    assert!(Face::new(vec![0u8; 64], 0).is_none());
}
