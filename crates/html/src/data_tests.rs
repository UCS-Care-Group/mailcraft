// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #533: pictures written into a message's HTML (`data:image/…;base64,…`).

use crate::{
    Block, BlockKind, MAX_DATA_PICTURE_BYTES, MAX_DATA_PICTURES, MAX_DATA_PICTURES_BYTES, MAX_INPUT, Picture, images, is_embedded, is_safe_url,
    parse, to_text,
};

/// A 1×1 PNG, made for this test (69 bytes: a whole number of base64 groups).
const PNG: [u8; 69] = [
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,
    0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53, 0xde, 0x00, 0x00, 0x00, 0x0c, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8, 0xcf, 0xc0, 0x00,
    0x00, 0x03, 0x01, 0x01, 0x00, 0xc9, 0xfe, 0x92, 0xef, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

/// Base64 (RFC 4648, with padding).
fn b64(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for group in bytes.chunks(3) {
        let n = group.iter().enumerate().fold(0u32, |n, (i, b)| n | u32::from(*b) << (16 - 8 * i));
        for i in 0..4 {
            if i <= group.len() {
                out.push(char::from(ABC[(n >> (18 - 6 * i) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// The pictures of `html`: the address kept, and the bytes when it is shown.
fn pictures(html: &str) -> Vec<(String, Option<Picture>)> {
    images(&parse(html))
        .into_iter()
        .filter_map(|b| match &b.kind {
            BlockKind::Image { src, data, .. } => Some((src.clone(), data.clone())),
            _ => None,
        })
        .collect()
}

fn img(src: &str) -> String {
    format!("<p>Before</p><img src=\"{src}\" alt=\"Chart\"><p>After</p>")
}

/// A PNG of `len` bytes or a little more: the real header, then zeros.
fn png_of(len: usize) -> String {
    let zeros = len.saturating_sub(PNG.len()).div_ceil(3) * 4;
    format!("data:image/png;base64,{}{}", b64(&PNG), "A".repeat(zeros))
}

#[test]
fn a_picture_written_into_the_message_is_shown() {
    let shown = pictures(&img(&format!("data:image/png;base64,{}", b64(&PNG))));
    assert_eq!(shown.len(), 1);
    let (src, data) = &shown[0];
    // The address kept says what it is, never its content.
    assert_eq!(src, "data:image/png");
    assert!(is_embedded(src));
    assert_eq!(data.as_deref(), Some(&PNG[..]));
    // Its description stays with it.
    let blocks = parse(&img(&format!("data:image/png;base64,{}", b64(&PNG))));
    let picture: Vec<&Block> = images(&blocks);
    assert_eq!(picture[0].text(), "Chart");
    // Debugged, the bytes are their length alone.
    let debugged = format!("{blocks:?}");
    assert!(debugged.contains("Picture(69 bytes)") && !debugged.contains("137, 80"), "{debugged}");
}

#[test]
fn the_scheme_and_type_in_any_letter_case_with_parameters() {
    for src in [
        format!("DATA:Image/PNG;BASE64,{}", b64(&PNG)),
        format!("  data:image/png;name=chart.png;base64,{}", b64(&PNG)),
        format!("da\nta:image/png;base64,{}", b64(&PNG)),
        // A JPEG's type on a PNG's bytes: the bytes say what it is.
        format!("data:image/jpeg;base64,{}", b64(&PNG)),
    ] {
        let shown = pictures(&img(&src));
        assert_eq!(shown.first().and_then(|(_, d)| d.as_deref()), Some(&PNG[..]), "{src}");
    }
}

#[test]
fn line_breaks_and_spaces_inside_the_base64_are_skipped() {
    // Quoted-printable leaves line breaks every 76 characters; some writers add spaces.
    let text = b64(&PNG);
    let broken: String = text.as_bytes().chunks(20).map(|c| String::from_utf8_lossy(c).into_owned()).collect::<Vec<_>>().join("\r\n ");
    let shown = pictures(&img(&format!("data:image/png;base64,{broken}")));
    assert_eq!(shown.first().and_then(|(_, d)| d.as_deref()), Some(&PNG[..]));
    // Without its padding, too.
    let shown = pictures(&img(&format!("data:image/png;base64,{}", b64(&PNG[..68]).trim_end_matches('='))));
    assert_eq!(shown.first().and_then(|(_, d)| d.as_deref()), Some(&PNG[..68]));
}

#[test]
fn what_is_not_a_picture_of_ours_is_a_placeholder() {
    let gif = b64(b"GIF89a\x01\x00\x01\x00\x00\x00\x00;");
    for src in [
        // Bad base64.
        "data:image/png;base64,iVBOR*w0KGgo".to_string(),
        format!("data:image/png;base64,{}=AAAA", b64(&PNG)),
        // Not base64 at all.
        "data:image/png,%89PNG".to_string(),
        // A type that isn't a picture of ours, whatever the bytes.
        format!("data:image/svg+xml;base64,{}", b64(b"<svg xmlns='http://www.w3.org/2000/svg'><script>x</script></svg>")),
        format!("data:text/html;base64,{}", b64(&PNG)),
        format!("data:;base64,{}", b64(&PNG)),
        format!("data:application/octet-stream;base64,{}", b64(&PNG)),
        // A picture's type on bytes that are no picture.
        format!("data:image/png;base64,{}", b64(b"<html><body>hello</body></html>")),
        "data:image/png;base64,".to_string(),
        "data:".to_string(),
        "data:image/png;base64".to_string(),
    ] {
        let shown = pictures(&img(&src));
        assert_eq!(shown.len(), 1, "{src}: a placeholder stands for it");
        let (kept, data) = &shown[0];
        assert!(data.is_none(), "{src}");
        assert!(kept.starts_with("data:") && kept.len() <= 20 && is_embedded(kept), "{src}: {kept}");
    }
    // A GIF is one of ours.
    assert!(pictures(&img(&format!("data:image/gif;base64,{gif}")))[0].1.is_some());
}

#[test]
fn a_data_address_is_never_a_link_or_a_background() {
    assert!(!is_safe_url(&format!("data:image/png;base64,{}", b64(&PNG))));
    let blocks = parse(&format!("<a href=\"data:image/png;base64,{}\">open</a>", b64(&PNG)));
    assert!(blocks.iter().flat_map(|b| &b.spans).all(|s| s.link.is_none()), "{blocks:?}");
    let blocks = parse(&format!("<a href=\"data:text/html,<script>x</script>\"><img src=\"data:image/png;base64,{}\"></a>", b64(&PNG)));
    let shown: Vec<_> = images(&blocks).into_iter().map(|b| &b.kind).collect();
    assert!(matches!(shown.as_slice(), [BlockKind::Image { link: None, data: Some(_), .. }]), "{shown:?}");
    // A background in the style or the old attribute is no picture.
    let blocks = parse(&format!(
        "<table background=\"data:image/png;base64,{0}\"><tr><td style=\"background:url(data:image/png;base64,{0})\">x</td></tr></table>",
        b64(&PNG)
    ));
    assert!(images(&blocks).is_empty());
}

#[test]
fn a_picture_over_the_cap_is_a_placeholder() {
    let over = png_of(MAX_DATA_PICTURE_BYTES + 4);
    let under = png_of(MAX_DATA_PICTURE_BYTES - 64);
    let shown = pictures(&format!("{}{}", img(&over), img(&under)));
    assert_eq!(shown.len(), 2);
    assert!(shown[0].1.is_none());
    assert!(shown[1].1.as_ref().is_some_and(|d| d.len() <= MAX_DATA_PICTURE_BYTES && d.starts_with(&PNG)));
}

#[test]
fn a_message_gets_only_so_many_and_so_much() {
    // Count.
    let one = img(&format!("data:image/png;base64,{}", b64(&PNG)));
    let shown = pictures(&one.repeat(MAX_DATA_PICTURES + 3));
    assert_eq!(shown.len(), MAX_DATA_PICTURES + 3);
    assert!(shown[..MAX_DATA_PICTURES].iter().all(|(_, d)| d.is_some()));
    assert!(shown[MAX_DATA_PICTURES..].iter().all(|(_, d)| d.is_none()));
    // Size: as many large ones as fit in the total.
    let each = MAX_DATA_PICTURES_BYTES / 5 + 65_536;
    let fit = MAX_DATA_PICTURES_BYTES / each;
    let html = img(&png_of(each)).repeat(fit + 1);
    let shown = pictures(&html);
    assert_eq!(shown.len(), fit + 1);
    assert!(shown[..fit].iter().all(|(_, d)| d.is_some()));
    assert!(shown[fit].1.is_none());
    let total: usize = shown.iter().filter_map(|(_, d)| d.as_ref()).map(|d| d.len()).sum();
    assert!(total <= MAX_DATA_PICTURES_BYTES);
}

#[test]
fn the_pictures_do_not_use_up_the_text() {
    // A message with a picture larger than the input read is still read to its end.
    let html = format!("{}<p>The end</p>", img(&png_of(MAX_INPUT + 1024)));
    let blocks = parse(&html);
    assert!(blocks.iter().any(|b| b.text() == "The end"), "{}", blocks.len());
    assert!(images(&blocks).first().is_some_and(|b| matches!(&b.kind, BlockKind::Image { data: Some(_), .. })));
    // Text is still cut where it always was.
    let long = format!("<p>{}</p><p>Past the end</p>", "a".repeat(MAX_INPUT));
    let blocks = parse(&long);
    assert!(!blocks.iter().any(|b| b.text().contains("Past")));
    assert!(blocks.first().is_some_and(|b| b.text().len() >= MAX_INPUT - 16));
}

#[test]
fn plain_text_leaves_the_pictures_alone() {
    let text = to_text(&img(&format!("data:image/png;base64,{}", b64(&PNG))));
    assert!(text.contains("Before") && text.contains("After") && !text.contains("iVBOR"), "{text}");
}

#[test]
fn a_hidden_or_tiny_picture_written_into_the_message_is_dropped() {
    let src = format!("data:image/png;base64,{}", b64(&PNG));
    for html in [
        format!("<img src=\"{src}\" width=1 height=1>"),
        format!("<img src=\"{src}\" style=\"display:none\">"),
        format!("<div hidden><img src=\"{src}\"></div>"),
    ] {
        assert!(pictures(&html).is_empty(), "{html}");
    }
}

proptest::proptest! {
    #[test]
    fn any_data_address_parses(src in "(data|DATA|da\ta):(image/(png|jpeg|gif|webp|svg\\+xml)|text/html)?(;[a-z=]{0,6}){0,2}(;base64)?,[A-Za-z0-9+/= \r\n%*-]{0,64}") {
        for (kept, data) in pictures(&img(&src)) {
            proptest::prop_assert!(is_embedded(&kept) && kept.len() <= 20);
            if let Some(data) = data {
                proptest::prop_assert!(data.len() <= 48);
            }
        }
    }
}
