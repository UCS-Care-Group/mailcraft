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
        pictures: Default::default(),
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
    let (bytes, pages) = pdf(&[doc(SAMPLE)], &fonts(), &Options::default()).unwrap();
    assert_eq!(pages, 1);
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
    d.pictures = [("cid:logo@example.org".to_string(), png(40, 20, true).into()), ("cid:photo@example.org".to_string(), jpeg(64, 48).into())].into();
    let (bytes, _) = pdf(&[d.clone()], &fonts(), &Options::default()).unwrap();
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
    let (bytes, _) = pdf(&[d], &f, &Options::default()).unwrap();
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
    let (bytes, _) = pdf(&[doc("<p>x</p>")], &fonts(), &Options { paper: Paper::LETTER, ..Options::default() }).unwrap();
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
        d.pictures = [("cid:big".to_string(), png(3000, 2, false).into()), ("cid:junk".to_string(), vec![0u8; 100].into())].into();
        let pages = page_text(&[d.clone()], &fonts(), &Options::default()).unwrap();
        assert!(!pages.is_empty() && pages.len() <= MAX_PAGES);
        assert!(pdf(&[d], &fonts(), &Options::default()).unwrap().0.starts_with(b"%PDF"));
    }
    assert_eq!(pdf(&[], &fonts(), &Options::default()), Err(Error::Empty));
    assert!(Face::new(vec![0u8; 64], 0).is_none());
}

/// The PDF's compressed streams, inflated and joined.
fn inflated(bytes: &[u8]) -> String {
    use std::io::Read;
    let mut out = String::new();
    let mut rest = bytes;
    while let Some(at) = rest.windows(7).position(|w| w == b"stream\n") {
        rest = &rest[at + 7..];
        let end = rest.windows(9).position(|w| w == b"endstream").unwrap_or(rest.len());
        let mut text = Vec::new();
        if flate2::read::ZlibDecoder::new(&rest[..end]).read_to_end(&mut text).is_ok() {
            out.push_str(&String::from_utf8_lossy(&text));
        }
        rest = &rest[end..];
    }
    out
}

/// An 8 × 8 CMYK JPEG (four components, Adobe marker), made for this test.
const CMYK_JPEG: &[u8] = &[
    0xff, 0xd8, 0xff, 0xee, 0x00, 0x0e, 0x41, 0x64, 0x6f, 0x62, 0x65, 0x00, 0x64, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff, 0xdb, 0x00, 0x43, 0x00, 0x03,
    0x02, 0x02, 0x03, 0x02, 0x02, 0x03, 0x03, 0x03, 0x03, 0x04, 0x03, 0x03, 0x04, 0x05, 0x08, 0x05, 0x05, 0x04, 0x04, 0x05, 0x0a, 0x07, 0x07, 0x06,
    0x08, 0x0c, 0x0a, 0x0c, 0x0c, 0x0b, 0x0a, 0x0b, 0x0b, 0x0d, 0x0e, 0x12, 0x10, 0x0d, 0x0e, 0x11, 0x0e, 0x0b, 0x0b, 0x10, 0x16, 0x10, 0x11, 0x13,
    0x14, 0x15, 0x15, 0x15, 0x0c, 0x0f, 0x17, 0x18, 0x16, 0x14, 0x18, 0x12, 0x14, 0x15, 0x14, 0xff, 0xc0, 0x00, 0x14, 0x08, 0x00, 0x08, 0x00, 0x08,
    0x04, 0x43, 0x11, 0x00, 0x4d, 0x11, 0x00, 0x59, 0x11, 0x00, 0x4b, 0x11, 0x00, 0xff, 0xc4, 0x00, 0x1f, 0x00, 0x00, 0x01, 0x05, 0x01, 0x01, 0x01,
    0x01, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0xff, 0xc4,
    0x00, 0xb5, 0x10, 0x00, 0x02, 0x01, 0x03, 0x03, 0x02, 0x04, 0x03, 0x05, 0x05, 0x04, 0x04, 0x00, 0x00, 0x01, 0x7d, 0x01, 0x02, 0x03, 0x00, 0x04,
    0x11, 0x05, 0x12, 0x21, 0x31, 0x41, 0x06, 0x13, 0x51, 0x61, 0x07, 0x22, 0x71, 0x14, 0x32, 0x81, 0x91, 0xa1, 0x08, 0x23, 0x42, 0xb1, 0xc1, 0x15,
    0x52, 0xd1, 0xf0, 0x24, 0x33, 0x62, 0x72, 0x82, 0x09, 0x0a, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x25, 0x26, 0x27, 0x28, 0x29, 0x2a, 0x34, 0x35, 0x36,
    0x37, 0x38, 0x39, 0x3a, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4a, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5a, 0x63, 0x64, 0x65, 0x66,
    0x67, 0x68, 0x69, 0x6a, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7a, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89, 0x8a, 0x92, 0x93, 0x94, 0x95,
    0x96, 0x97, 0x98, 0x99, 0x9a, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7, 0xa8, 0xa9, 0xaa, 0xb2, 0xb3, 0xb4, 0xb5, 0xb6, 0xb7, 0xb8, 0xb9, 0xba, 0xc2,
    0xc3, 0xc4, 0xc5, 0xc6, 0xc7, 0xc8, 0xc9, 0xca, 0xd2, 0xd3, 0xd4, 0xd5, 0xd6, 0xd7, 0xd8, 0xd9, 0xda, 0xe1, 0xe2, 0xe3, 0xe4, 0xe5, 0xe6, 0xe7,
    0xe8, 0xe9, 0xea, 0xf1, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8, 0xf9, 0xfa, 0xff, 0xda, 0x00, 0x0e, 0x04, 0x43, 0x00, 0x4d, 0x00, 0x59, 0x00,
    0x4b, 0x00, 0x00, 0x3f, 0x00, 0xfd, 0x53, 0xaf, 0xca, 0xaa, 0xfc, 0xaa, 0xaf, 0xd5, 0x3a, 0xff, 0xd9,
];

fn cmyk_jpeg() -> Vec<u8> {
    CMYK_JPEG.to_vec()
}

#[test]
fn only_grey_and_ycbcr_jpegs_are_passed_through() {
    assert_eq!(picture::passes_through(&jpeg(8, 8)), Some(false));
    let mut gray = std::io::Cursor::new(Vec::new());
    image::GrayImage::from_pixel(8, 8, image::Luma([90])).write_to(&mut gray, image::ImageFormat::Jpeg).unwrap();
    assert_eq!(picture::passes_through(gray.get_ref()), Some(true));
    assert_eq!(picture::passes_through(&cmyk_jpeg()), None, "CMYK is decoded and written again");
    // An Adobe JPEG that says its three components are RGB, not YCbCr.
    let mut rgb = jpeg(8, 8);
    let mut adobe = vec![0xFF, 0xEE, 0x00, 0x0E];
    adobe.extend_from_slice(b"Adobe\x00\x64\x00\x00\x00\x00\x00");
    rgb.splice(2..2, adobe);
    assert_eq!(picture::passes_through(&rgb), None);
    for junk in [&b""[..], b"\xFF\xD8", b"\xFF\xD8\xFF\xC0\x00", b"\xFF\xD8\xFF\xE0\xFF\xFF"] {
        assert_eq!(picture::passes_through(junk), None);
    }
}

#[test]
fn a_picture_is_decoded_once_however_often_it_is_shown_and_only_kinds_the_pane_shows_print() {
    let shared: Arc<Vec<u8>> = png(30, 30, false).into();
    let html: String = (0..300).map(|n| format!(r#"<img src="cid:Logo{n}@x">"#)).collect();
    let mut d = doc(&html);
    d.pictures = (0..300).map(|n| (format!("cid:Logo{n}@x"), shared.clone())).collect();
    let (bytes, _) = pdf(&[d], &fonts(), &Options::default()).unwrap();
    assert_eq!(String::from_utf8_lossy(&bytes).matches("/Subtype /Image").count(), 1);
    let mut ico = std::io::Cursor::new(Vec::new());
    image::RgbaImage::from_pixel(8, 8, image::Rgba([1, 2, 3, 255])).write_to(&mut ico, image::ImageFormat::Ico).unwrap();
    let mut d = doc(r#"<img src="cid:t" alt="icon">"#);
    d.pictures = [("cid:t".to_string(), ico.into_inner().into())].into();
    let (bytes, _) = pdf(&[d], &fonts(), &Options::default()).unwrap();
    assert_eq!(String::from_utf8_lossy(&bytes).matches("/Subtype /Image").count(), 0, "an icon file isn't shown in the pane");
    let mut d = doc(r#"<img src="cid:c">"#);
    d.pictures = [("cid:c".to_string(), cmyk_jpeg().into())].into();
    let (bytes, _) = pdf(&[d], &fonts(), &Options::default()).unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert_eq!(text.matches("/Subtype /Image").count(), 1, "the CMYK picture prints");
    assert!(!text.contains("/DCTDecode"), "decoded and written again, never tagged RGB");
}

#[test]
fn a_character_no_face_has_says_what_it_is() {
    let d = doc("<p>A \u{4e2d} B \u{65e5} C</p>");
    let (bytes, _) = pdf(&[d], &fonts(), &Options::default()).unwrap();
    assert!(inflated(&bytes).contains("/ActualText"), "the run with the missing glyph carries its text");
    assert!(page_text(&[doc("<p>A \u{4e2d} B \u{65e5} C</p>")], &fonts(), &Options::default()).unwrap()[0].contains("A \u{4e2d} B \u{65e5} C"));
}

#[test]
fn a_line_of_words_is_one_piece_of_text() {
    let mut faces = font::FaceSet::new(&fonts()).unwrap();
    let mut pictures = picture::Pictures::default();
    let d = doc("<p>one two three four five six seven</p>");
    let pages = layout::pages(&[d], &mut faces, &mut pictures, &Options::default());
    let runs = pages[0].ops.iter().filter(|op| matches!(op, layout::Op::Text { text, .. } if text.contains("one"))).count();
    let whole = pages[0].ops.iter().any(|op| matches!(op, layout::Op::Text { text, .. } if text == "one two three four five six seven"));
    assert!(runs == 1 && whole);
}

#[test]
fn a_row_taller_than_a_page_keeps_its_cells_in_place_with_their_colours() {
    let html = format!(r#"<table><tr><td style="background:#e8f0fe">{}</td><td>right side</td></tr></table>"#, "<p>line</p>".repeat(200));
    let mut faces = font::FaceSet::new(&fonts()).unwrap();
    let mut pictures = picture::Pictures::default();
    let pages = layout::pages(&[doc(&html)], &mut faces, &mut pictures, &Options::default());
    assert!(pages.len() >= 3);
    for page in &pages[1..] {
        assert!(page.ops.iter().any(|op| matches!(op, layout::Op::Rect { .. })), "the cell's colour goes on with it");
    }
    let ops: Vec<&layout::Op> = pages.iter().flat_map(|p| &p.ops).collect();
    let right = ops.iter().find_map(|op| match op {
        layout::Op::Text { x, text, .. } if text.contains("right") => Some(*x),
        _ => None,
    });
    let left = ops.iter().find_map(|op| match op {
        layout::Op::Text { x, text, .. } if text == "line" => Some(*x),
        _ => None,
    });
    assert!(right.unwrap() > left.unwrap() + 10.0, "the second cell stays beside the first");
}

#[test]
fn what_is_past_the_last_page_is_never_laid_out() {
    let body = format!("<pre>{}</pre>", "a line\n".repeat(MAX_PAGES * 70));
    let started = std::time::Instant::now();
    let (_, pages) = pdf(&[doc(&body), doc("<p>never</p>")], &fonts(), &Options::default()).unwrap();
    assert_eq!(pages, MAX_PAGES);
    assert!(started.elapsed() < std::time::Duration::from_secs(60));
}

#[test]
fn a_picture_one_message_had_no_room_for_prints_in_the_next() {
    // The first message fills its 500 pictures; its logo after them has no room there.
    let html: String = (0..500).map(|n| format!(r#"<img src="cid:p{n}">"#)).collect::<String>() + r#"<img src="cid:logo">"#;
    let mut first = doc(&html);
    first.pictures = (0..500u32)
        .map(|n| {
            let mut out = std::io::Cursor::new(Vec::new());
            image::RgbImage::from_pixel(2, 2, image::Rgb([(n % 256) as u8, (n / 256) as u8, 7])).write_to(&mut out, image::ImageFormat::Png).unwrap();
            (format!("cid:p{n}"), Arc::new(out.into_inner()))
        })
        .collect();
    let logo: Arc<Vec<u8>> = Arc::new(png(20, 20, false));
    first.pictures.insert("cid:logo".into(), logo.clone());
    let mut second = doc(r#"<img src="cid:logo">"#);
    second.pictures.insert("cid:logo".into(), logo);
    let mut faces = font::FaceSet::new(&fonts()).unwrap();
    let mut pictures = picture::Pictures::default();
    let pages = layout::pages(&[first, second], &mut faces, &mut pictures, &Options::default());
    let last = pages.last().unwrap();
    assert!(last.ops.iter().any(|op| matches!(op, layout::Op::Image { .. })), "the logo is in the second message");
}
