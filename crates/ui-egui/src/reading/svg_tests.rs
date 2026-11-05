// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #565: a message's SVG pictures are drawn, with nothing they refer to ever fetched or read,
//! and a drawing made to take the window down (by its size, its depth, its references or its
//! filters) is a broken picture, quickly.

use std::time::{Duration, Instant};

use mailcraft_engine::Session;
use mailcraft_model::{Address, Attachment, Body, Mailbox, Message};
use serde_json::json;

use super::images::{self, Pictures};
use super::svg::{self, MAX_SVG_BYTES, MAX_SVG_DEPTH, MAX_SVG_PIXELS, MAX_SVG_SIDE};
use crate::{MailApp, Services};

const NOW: i64 = 1_791_993_600;
const NS: &str = "xmlns='http://www.w3.org/2000/svg' xmlns:xlink='http://www.w3.org/1999/xlink'";
const GREEN: [u8; 4] = [0x2e, 0x8b, 0x57, 0xff];

/// A drawing `w`×`h` of `body`, on a green ground.
fn drawing(w: u32, h: u32, body: &str) -> Vec<u8> {
    format!("<svg {NS} width='{w}' height='{h}'><rect width='{w}' height='{h}' fill='#2e8b57'/>{body}</svg>").into_bytes()
}

fn pixel(image: &egui::ColorImage, x: usize, y: usize) -> [u8; 4] {
    image.pixels[y * image.size[0] + x].to_array()
}

/// Whether any pixel of the image is red (what every file or address a test offers is drawn in).
fn any_red(image: &egui::ColorImage) -> bool {
    image.pixels.iter().any(|c| {
        let [r, g, b, _] = c.to_array();
        r > 0xc0 && g < 0x40 && b < 0x40
    })
}

/// Whether `data` is refused, quickly: no size, no picture.
fn refused(data: &[u8]) -> bool {
    let start = Instant::now();
    let refused = images::probe(data).is_none() && images::decode(data, 512, 512).is_none();
    assert!(start.elapsed() < Duration::from_secs(3), "took {:?}", start.elapsed());
    refused
}

#[test]
fn an_svg_is_drawn() {
    let data = drawing(20, 10, "");
    assert!(svg::is_svg(&data));
    assert_eq!(images::probe(&data), Some([20, 10]));
    let image = images::decode(&data, 2048, 20).unwrap();
    assert_eq!(image.size, [20, 10]);
    assert_eq!(pixel(&image, 10, 5), GREEN);
    // A viewBox alone gives the size; what is outside the drawing is clear.
    let data = format!("<svg {NS} viewBox='0 0 40 20'><circle cx='10' cy='10' r='8' fill='#2e8b57'/></svg>").into_bytes();
    assert_eq!(images::probe(&data), Some([40, 20]));
    let image = images::decode(&data, 2048, 40).unwrap();
    assert_eq!(pixel(&image, 10, 10), GREEN);
    assert_eq!(pixel(&image, 30, 10)[3], 0);
}

#[test]
fn an_svg_is_drawn_at_the_size_it_is_shown() {
    let data = drawing(20, 10, "");
    // Larger than its own size, as a drawing can be, and sharp: drawn at that size, not scaled.
    let image = images::decode(&data, 2048, 200).unwrap();
    assert_eq!(image.size, [200, 100]);
    assert_eq!(pixel(&image, 199, 99), GREEN);
    // Never past the largest texture, or the pixels a drawing may take.
    let image = images::decode(&data, 256, 4000).unwrap();
    assert_eq!(image.size, [256, 128]);
    let image = images::decode(&data, 8192, 8192).unwrap();
    assert!((image.size[0] * image.size[1]) as u64 <= MAX_SVG_PIXELS, "{:?}", image.size);
    // A drawing that says it is enormous is shown smaller, not refused.
    let huge = format!("<svg {NS} width='1e9' height='5e8'><rect width='1e9' height='5e8' fill='#2e8b57'/></svg>").into_bytes();
    let [w, h] = images::probe(&huge).unwrap();
    assert!(w as f32 <= MAX_SVG_SIDE && h as f32 <= MAX_SVG_SIDE && w == 2 * h, "{w}×{h}");
    assert_eq!(pixel(&images::decode(&huge, 2048, 64).unwrap(), 10, 10), GREEN);
}

/// A drawing that refers to `address` every way a drawing can, each drawn in red.
fn referring_to(address: &str) -> Vec<u8> {
    drawing(
        40,
        40,
        &format!(
            "<style>@import url('{address}/sheet.css'); @font-face {{ font-family: X; src: url('{address}/font.woff') }} \
             .far {{ fill: url('{address}/paint.svg#p') red }}</style>\
             <image href='{address}/a.svg' x='0' y='0' width='40' height='40'/>\
             <image xlink:href='{address}/b.svg' x='0' y='0' width='40' height='40'/>\
             <image href='{address}/c.png' x='0' y='0' width='40' height='40'/>\
             <use href='{address}/d.svg#r'/><use xlink:href='{address}/e.svg#r'/>\
             <rect width='40' height='40' fill='url({address}/f.svg#p) red' opacity='0' class='far'/>\
             <filter id='fi'><feImage href='{address}/g.svg'/></filter><rect width='40' height='40' filter='url(#fi)' fill='none'/>\
             <text x='1' y='20' font-family='X' fill='red'>words</text>"
        ),
    )
}

#[test]
fn nothing_an_svg_refers_to_on_the_web_is_fetched() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    for address in [address.clone(), address.replace("http:", "https:"), format!("//127.0.0.1:{}", listener.local_addr().unwrap().port())] {
        let data = referring_to(&address);
        let image = images::decode(&data, 2048, 40).unwrap();
        assert_eq!(pixel(&image, 20, 20), GREEN, "{address}");
    }
    // Nobody came: the drawing is drawn and done, and nothing was asked of the server.
    match listener.accept() {
        Err(e) => assert_eq!(e.kind(), std::io::ErrorKind::WouldBlock),
        Ok((_, from)) => panic!("an SVG made a connection from {from}"),
    }
}

#[test]
fn no_file_an_svg_refers_to_is_read() {
    // A red drawing on this computer, named every way a drawing could name it.
    let dir = std::env::temp_dir().join(format!("mailcraft-svg-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("red.svg");
    std::fs::write(&file, format!("<svg {NS} width='40' height='40'><rect width='40' height='40' fill='red'/></svg>")).unwrap();
    // As a drawing would load it (the default way would draw it in red: proved here, so that the
    // test below proves something).
    let path = file.to_string_lossy().replace('\\', "/");
    let names = [path.clone(), file.to_string_lossy().into_owned(), format!("file:///{}", path.trim_start_matches('/')), "red.svg".into()];
    let mut loads = resvg::usvg::Options { resources_dir: Some(dir.clone()), ..Default::default() };
    loads.image_href_resolver = resvg::usvg::ImageHrefResolver::default();
    let tree = resvg::usvg::Tree::from_data(&drawing(40, 40, &format!("<image href='{path}' width='40' height='40'/>")), &loads).unwrap();
    let mut pixmap = resvg::tiny_skia::Pixmap::new(40, 40).unwrap();
    resvg::render(&tree, resvg::tiny_skia::Transform::default(), &mut pixmap.as_mut());
    assert_eq!(pixmap.pixel(20, 20).map(|p| [p.red(), p.green(), p.blue()]), Some([255, 0, 0]), "the default resolver reads files");
    for name in names {
        for body in [
            format!("<image href='{name}' width='40' height='40'/>"),
            format!("<image xlink:href='{name}' width='40' height='40'/>"),
            format!("<filter id='f'><feImage href='{name}'/></filter><rect width='40' height='40' filter='url(#f)'/>"),
        ] {
            let image = images::decode(&drawing(40, 40, &body), 2048, 40).unwrap();
            assert!(!any_red(&image), "{body}");
        }
    }
    // Nor one written into it: a drawing never holds another, nor a picture.
    let red = b64(format!("<svg {NS} width='40' height='40'><rect width='40' height='40' fill='red'/></svg>").as_bytes());
    let image =
        images::decode(&drawing(40, 40, &format!("<image href='data:image/svg+xml;base64,{red}' width='40' height='40'/>")), 2048, 40).unwrap();
    assert!(!any_red(&image));
    let _ = std::fs::remove_dir_all(&dir);
}

fn b64(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    bytes
        .chunks(3)
        .flat_map(|group| {
            let n = group.iter().enumerate().fold(0u32, |n, (i, b)| n | u32::from(*b) << (16 - 8 * i));
            (0..4).map(move |i| if i <= group.len() { char::from(ABC[(n >> (18 - 6 * i) & 63) as usize]) } else { '=' })
        })
        .collect()
}

#[test]
fn words_in_an_svg_are_left_out_and_the_rest_drawn() {
    // No font is looked for: the words are not drawn, the shapes are.
    let data = drawing(40, 20, "<text x='0' y='18' font-size='30' fill='red'>MMMM</text>");
    let image = images::decode(&data, 2048, 40).unwrap();
    assert!(!any_red(&image));
    assert_eq!(pixel(&image, 20, 10), GREEN);
}

#[test]
fn what_an_svg_says_runs_never_does() {
    let data = drawing(
        20,
        20,
        "<script>while(true){}</script><rect width='20' height='20' fill='#2e8b57' onload='alert(1)'/>\
         <animate attributeName='fill' to='red' dur='1s'/><set attributeName='fill' to='red'/>\
         <foreignObject width='20' height='20'><div xmlns='http://www.w3.org/1999/xhtml' style='background:red'>x</div></foreignObject>",
    );
    let image = images::decode(&data, 2048, 20).unwrap();
    assert_eq!(pixel(&image, 10, 10), GREEN);
    assert!(!any_red(&image));
}

#[test]
fn entities_are_refused_and_a_plain_document_type_is_not() {
    let laughs = format!(
        "<?xml version='1.0'?><!DOCTYPE svg [<!ENTITY a 'lol'>{}]><svg {NS} width='1' height='1'><desc>&j;</desc></svg>",
        (b'b'..=b'j').map(|c| format!("<!ENTITY {} '{}'>", char::from(c), format!("&{};", char::from(c - 1)).repeat(10))).collect::<String>()
    );
    assert!(refused(laughs.as_bytes()));
    // One entity, however harmless, is refused: none is ever expanded.
    assert!(refused(format!("<!DOCTYPE svg [<!ENTITY ns 'http://www.w3.org/2000/svg'>]><svg {NS} width='1' height='1'/>").as_bytes()));
    assert!(refused(format!("<!DOCTYPE svg [<!entity x SYSTEM 'file:///etc/passwd'>]><svg {NS} width='1' height='1'>&x;</svg>").as_bytes()));
    // What drawing programs write before a drawing is fine.
    let plain = format!(
        "<?xml version='1.0' encoding='UTF-8'?>\n<!-- Generator: a drawing program -->\n<!DOCTYPE svg PUBLIC '-//W3C//DTD SVG 1.1//EN' \
         'http://www.w3.org/Graphics/SVG/1.1/DTD/svg11.dtd'>\n{}",
        String::from_utf8(drawing(8, 8, "")).unwrap()
    );
    assert_eq!(images::probe(plain.as_bytes()), Some([8, 8]));
}

#[test]
fn a_packed_svg_is_not_unpacked() {
    // svgz: what would be unpacked could be any size. Not an SVG by its bytes, and not read as one.
    let packed = b"\x1f\x8b\x08\x00\x00\x00\x00\x00\x00\x03\xb3\x29\x2e\x4b\x57\x00\x00";
    assert!(!svg::is_svg(packed));
    assert!(refused(packed));
}

#[test]
fn an_svg_over_the_caps_is_refused() {
    // Too many bytes.
    let big = drawing(8, 8, &format!("<desc>{}</desc>", "x".repeat(MAX_SVG_BYTES)));
    assert!(refused(&big));
    // Too many elements.
    assert!(refused(&drawing(8, 8, &"<g/>".repeat(30_000))));
    // Too deep, by nesting and by references in a chain.
    let deep = |n: usize| drawing(8, 8, &format!("{}{}", "<g>".repeat(n), "</g>".repeat(n)));
    assert!(refused(&deep(10_000)));
    assert!(refused(&deep(MAX_SVG_DEPTH + 1)));
    let chain: String = (0..MAX_SVG_DEPTH + 2).map(|i| format!("<use id='u{}' href='#u{i}'/>", i + 1)).collect();
    assert!(refused(&drawing(8, 8, &format!("<defs><rect id='u0' width='1' height='1'/>{chain}</defs>"))));
    // A drawing of no size, or not one at all.
    assert!(refused(format!("<svg {NS} width='0' height='10'/>").as_bytes()));
    assert!(refused(format!("<svg {NS} width='-5' height='10'/>").as_bytes()));
}

#[test]
fn an_svg_at_the_deepest_allowed_is_drawn_on_a_small_stack() {
    // The window's thread has the system's stack, a megabyte on Windows: the deepest drawing
    // allowed is drawn on one, in a build without optimisations at that.
    let depth = MAX_SVG_DEPTH - 2;
    let data = drawing(8, 8, &format!("{}<rect width='8' height='8' fill='#2e8b57'/>{}", "<g opacity='0.99'>".repeat(depth), "</g>".repeat(depth)));
    let drawn = std::thread::Builder::new().stack_size(1024 * 1024).spawn(move || images::decode(&data, 64, 8).map(|i| i.size)).unwrap().join();
    assert_eq!(drawn.ok().flatten(), Some([8, 8]));
}

#[test]
fn references_that_multiply_are_refused() {
    // Ten deep, each ten times the one before: 10^10 copies.
    let mut uses = String::from("<rect id='l0' width='1' height='1'/>");
    for level in 1..=10 {
        uses += &format!("<g id='l{level}'>{}</g>", format!("<use href='#l{}'/>", level - 1).repeat(10));
    }
    assert!(refused(&drawing(8, 8, &format!("<defs>{uses}</defs><use href='#l10'/>"))));
    assert!(refused(&drawing(8, 8, &format!("<defs>{}</defs><use xlink:href='#l10'/>", uses.replace("href=", "xlink:href=")))));
    // Patterns that fill with patterns, through a style sheet and through attributes.
    let mut patterns = String::from("<pattern id='p0' width='1' height='1'><rect width='1' height='1'/></pattern>");
    for level in 1..=10 {
        patterns += &format!(
            "<pattern id='p{level}' width='1' height='1'>{}</pattern>",
            format!("<rect width='1' height='1' class='c{}'/>", level - 1).repeat(10)
        );
    }
    let sheet: String = (0..=10).map(|i| format!(".c{i} {{ fill: url(#p{i}) }}")).collect();
    assert!(refused(&drawing(8, 8, &format!("<style>{sheet}</style><defs>{patterns}</defs><rect width='8' height='8' class='c10'/>"))));
    let attributes = patterns.replace(" class='c", " fill='url(#p").replace("'/>", ")'/>");
    assert!(refused(&drawing(8, 8, &format!("<defs>{attributes}</defs><rect width='8' height='8' fill='url(#p10)'/>"))));
    // Clips and masks the same way, a selector that matches everything, and a marker at each
    // of very many points.
    let mut masks = String::from("<mask id='m0'><rect width='1' height='1'/></mask>");
    for level in 1..=10 {
        masks += &format!(
            "<mask id='m{level}'>{}</mask>",
            "<rect width='1' height='1' style='mask: url(#mX)'/>".replace('X', &(level - 1).to_string()).repeat(10)
        );
    }
    assert!(refused(&drawing(8, 8, &format!("<defs>{masks}</defs><rect width='8' height='8' mask='url(#m10)'/>"))));
    assert!(refused(&drawing(8, 8, &format!("<style>* {{ clip-path: url(#c) }}</style><clipPath id='c'><rect width='1' height='1'/></clipPath>"))));
    let points: String = (0..20_000).map(|i| format!(" {} {}", i % 8, i % 7)).collect();
    let marker = format!("<marker id='mk'>{}</marker>", "<rect width='1' height='1'/>".repeat(100));
    assert!(refused(&drawing(8, 8, &format!("<defs>{marker}</defs><polyline points='{points}' marker-mid='url(#mk)'/>"))));
    // A reference to itself.
    assert!(refused(&drawing(8, 8, "<g id='me'><use href='#me'/></g>")));
}

#[test]
fn what_drawing_programs_write_is_not_mistaken_for_a_bomb() {
    // Gradients and clips from a style sheet, as drawing programs export them, many of each.
    let mut body = String::from("<defs><style>");
    for i in 0..60 {
        body += &format!(".f{i}{{fill:url(#g{i});}}.k{i}{{clip-path:url(#c{i});}}");
    }
    body += "</style>";
    for i in 0..60 {
        body += &format!(
            "<linearGradient id='g{i}' x1='0' x2='1'><stop offset='0' stop-color='#2e8b57'/><stop offset='1' stop-color='#2e8b57'/></linearGradient>\
             <clipPath id='c{i}'><rect width='40' height='40'/></clipPath>"
        );
    }
    body += "</defs>";
    for i in 0..60 {
        body += &format!("<g class='k{i}'><path class='f{i}' d='M0 0h40v40H0z'/><use href='#s'/></g>");
    }
    body += "<symbol id='s'><circle r='1' fill='#2e8b57'/></symbol>";
    // A drop shadow.
    body += "<filter id='sh'><feGaussianBlur stdDeviation='2'/><feOffset dx='1' dy='1'/><feMerge><feMergeNode/><feMergeNode in='SourceGraphic'/></feMerge></filter>\
             <rect width='10' height='10' fill='#2e8b57' filter='url(#sh)'/>";
    let image = images::decode(&drawing(40, 40, &body), 2048, 40).unwrap();
    assert_eq!(pixel(&image, 30, 30), GREEN);
}

#[test]
fn filters_that_cost_without_bound_are_refused() {
    for filter in [
        "<feMorphology radius='1'/>",
        "<feTurbulence baseFrequency='0.05' numOctaves='1000000000'/>",
        "<feConvolveMatrix order='40' kernelMatrix='1'/>",
    ] {
        let data = drawing(8, 8, &format!("<filter id='f'>{filter}</filter><rect width='8' height='8' filter='url(#f)'/>"));
        assert!(refused(&data), "{filter}");
    }
    // Within bounds they are drawn.
    let data =
        drawing(8, 8, "<filter id='f'><feTurbulence baseFrequency='0.05' numOctaves='4'/></filter><rect width='8' height='8' filter='url(#f)'/>");
    assert!(images::decode(&data, 64, 8).is_some());
}

#[test]
fn a_drawing_of_many_layers_is_drawn_with_fewer_pixels() {
    let many: String = (0..800).map(|i| format!("<rect x='{}' width='1' height='40' fill='#2e8b57' opacity='0.5'/>", i % 40)).collect();
    let few = svg::probe(&drawing(40, 40, "")).unwrap();
    let layered = svg::probe(&drawing(40, 40, &many)).unwrap();
    assert!(layered.max_pixels < few.max_pixels, "{} {}", layered.max_pixels, few.max_pixels);
    let image = images::decode(&drawing(40, 40, &many), 8192, 8192).unwrap();
    assert!((image.size[0] * image.size[1]) as u64 <= layered.max_pixels, "{:?}", image.size);
}

#[test]
fn a_damaged_svg_is_a_broken_picture() {
    for data in [
        format!("<svg {NS} width='8' height='8'><rect width='8' height='8'></svg>").into_bytes(),
        format!("<svg {NS} width='8' height='8'>").into_bytes(),
        b"<svg width='8' height='8'><rect width='8' height='8'/></svg>".to_vec(),
        b"<svg xmlns='http://www.w3.org/1999/xhtml' width='8' height='8'/>".to_vec(),
        [&b"<svg xmlns='http://www.w3.org/2000/svg' width='8' height='8'><desc>"[..], &[0xff, 0xfe, 0xc0], b"</desc></svg>"].concat(),
        b"<svg".to_vec(),
    ] {
        assert!(refused(&data), "{}", String::from_utf8_lossy(&data));
    }
    // What is said of it, when it came from the web.
    assert_eq!(images::not_a_picture(b"<svg viewBox='0 0 1 1'></svg>"), "it is a drawing (SVG) that is damaged or can't be shown");
}

/// A small generator of the same numbers every run (xorshift).
struct Junk(u64);

impl Junk {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

#[test]
fn arbitrary_and_damaged_svgs_never_panic() {
    let mut junk = Junk(0x5851_f42d_4c95_7f2d);
    let good = drawing(
        16,
        16,
        "<defs><linearGradient id='g'><stop offset='0' stop-color='red'/></linearGradient><clipPath id='c'><circle r='4'/></clipPath>\
         <filter id='f'><feGaussianBlur stdDeviation='1'/></filter></defs>\
         <path d='M1 1 L15 15 Q 8 0 1 15 Z' fill='url(#g)' clip-path='url(#c)' filter='url(#f)' stroke-dasharray='1 2' stroke='blue'/>\
         <use href='#c'/><style>.a{fill:url(#g)}</style>",
    );
    let pieces = ["<", ">", "/", "'", "=", " ", "<g>", "</g>", "url(#", "#g", "href='#c'", "<!--", "]]>", "<![CDATA[", "&amp;", "&#x0;", "e9", "-"];
    for round in 0..600 {
        let mut data = if round % 3 == 0 { format!("<svg {NS} ").into_bytes() } else { good.clone() };
        for _ in 0..1 + junk.below(6) {
            let at = junk.below(data.len() + 1);
            if junk.below(2) == 0 {
                let piece = pieces[junk.below(pieces.len())].as_bytes();
                data.splice(at..at, piece.iter().copied());
            } else if at < data.len() {
                data[at] = junk.next() as u8;
            }
        }
        if round % 5 == 0 {
            let keep = junk.below(data.len());
            data.truncate(keep);
        }
        let _ = images::probe(&data);
        if let Some(image) = images::decode(&data, 128, 128) {
            assert!(image.size[0] <= 128 && image.size[1] <= 128);
        }
    }
}

/// A message with one picture, `src`, and the attachment it may refer to.
fn message(src: &str, attachment: Option<Vec<u8>>) -> Message {
    Message {
        from: Address::new("A", "a@example.org"),
        to: vec![Address::new("Avery Quinn", "avery@lumen-works.example")],
        subject: "A drawing".into(),
        date: NOW - 3_600,
        received: NOW - 3_600,
        read: true,
        body: Body { text: String::new(), html: Some(format!("<p>Before</p><img src=\"{src}\" width=40 height=20><p>After</p>")) },
        attachments: attachment
            .map(|data| {
                vec![Attachment { content_id: "logo@example.org".into(), inline: true, ..Attachment::new("logo.svg", "image/svg+xml", data) }]
            })
            .unwrap_or_default(),
        ..Message::default()
    }
}

/// How many pictures the window keeps and how many it has a texture for, once `m` is shown.
fn shown(m: Message) -> (usize, usize) {
    let mut mailbox = Mailbox::with_local_account("Avery Quinn", "avery@lumen-works.example");
    let inbox = mailbox.inbox().unwrap();
    let mut m = m;
    m.folder = inbox;
    let id = mailbox.add_message(m).unwrap();
    let mut app = MailApp::new(Session::at(mailbox, NOW, 0), Services::default());
    app.autosave = false;
    app.auto_sync = false;
    let mut harness =
        egui_kittest::Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(1280.0, 800.0)).build_ui_state(
            |ui, app: &mut MailApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            app,
        );
    harness.run_steps(3);
    harness.state_mut().execute("mail.select", json!({"id": id})).unwrap();
    harness.run_steps(20);
    let pictures = Pictures::of(&harness.ctx);
    let pictures = pictures.lock().unwrap();
    pictures.counts()
}

#[test]
fn an_attached_svg_is_shown_in_the_message() {
    assert_eq!(shown(message("cid:logo@example.org", Some(drawing(40, 20, "")))), (1, 1));
    // A damaged one is a stand-in.
    assert_eq!(shown(message("cid:logo@example.org", Some(b"<svg><rect></svg>".to_vec()))).1, 0);
}

#[test]
fn an_svg_written_into_the_message_is_shown_in_it() {
    let src = format!("data:image/svg+xml;base64,{}", b64(&drawing(40, 20, "")));
    assert_eq!(shown(message(&src, None)), (1, 1));
}
