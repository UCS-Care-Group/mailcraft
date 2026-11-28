// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Saving a picture a message shows (#1420): a part, a picture written into the message, and
//! bytes already kept of one on the web. Nothing is fetched.

use mailcraft_model::{Attachment, Body, Mailbox, Message};
use serde_json::{Value, json};

use crate::{Session, cmd};

fn png(w: u32, h: u32, rgb: [u8; 3]) -> Vec<u8> {
    encode(image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(w, h, image::Rgba([rgb[0], rgb[1], rgb[2], 255]))), image::ImageFormat::Png)
}

fn jpeg(w: u32, h: u32) -> Vec<u8> {
    let rgb = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(w, h, image::Rgba([20, 40, 60, 255]))).into_rgb8();
    encode(image::DynamicImage::ImageRgb8(rgb), image::ImageFormat::Jpeg)
}

fn gif(w: u32, h: u32) -> Vec<u8> {
    encode(image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(w, h, image::Rgba([10, 180, 40, 255]))), image::ImageFormat::Gif)
}

fn webp(w: u32, h: u32) -> Vec<u8> {
    encode(image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(w, h, image::Rgba([200, 30, 30, 255]))), image::ImageFormat::WebP)
}

fn encode(image: image::DynamicImage, format: image::ImageFormat) -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    image.write_to(&mut out, format).unwrap();
    out.into_inner()
}

fn b64(bytes: &[u8]) -> String {
    cmd::base64_encode(bytes)
}

fn hold(html: &str, attachments: Vec<Attachment>) -> (Session, u64) {
    let mut mailbox = Mailbox::with_local_account("Avery Quinn", "avery@example.org");
    let inbox = mailbox.inbox().unwrap();
    let id = mailbox
        .add_message(Message {
            folder: inbox,
            subject: "A picture".into(),
            body: Body { html: Some(html.to_string()), text: String::new() },
            attachments,
            ..Message::default()
        })
        .unwrap();
    let mut session = Session::at(mailbox, 1_763_596_800, 0);
    session.run("mail.select", &json!({"id": id})).unwrap();
    (session, id)
}

fn part(name: &str, id: &str, bytes: Vec<u8>) -> Attachment {
    Attachment { content_id: id.to_string(), inline: true, ..Attachment::new(name, "application/octet-stream", bytes) }
}

fn save(session: &mut Session, params: Value) -> Value {
    session.run("mail.savePicture", &params).unwrap_or_else(|e| panic!("{params}: {e}"))
}

fn refuse(session: &mut Session, params: Value) -> String {
    session.run("mail.savePicture", &params).unwrap_err().to_string()
}

fn bytes_of(value: &Value) -> Vec<u8> {
    cmd::base64_decode(value["data"].as_str().unwrap()).unwrap()
}

#[test]
fn a_part_the_message_shows_is_saved_as_its_own_bytes() {
    let bytes = png(12, 8, [1, 2, 3]);
    let (mut session, id) =
        hold(r#"<p>See</p><img src="cid:shot@example" alt="ridge">"#, vec![part("Quarterly Report.PNG", "shot@example", bytes.clone())]);
    let got = save(&mut session, json!({"id": id, "src": "cid:shot@example"}));
    assert_eq!((got["name"].as_str(), bytes_of(&got).as_slice()), (Some("Quarterly Report.PNG"), bytes.as_slice()));
    assert!(!session.can_undo());
    // A name whose extension is not the picture's is given the one the bytes are.
    let (mut session, id) = hold(r#"<img src="cid:scan@example">"#, vec![part("scan.bin", "scan@example", bytes.clone())]);
    assert_eq!(save(&mut session, json!({"src": "cid:scan@example"}))["name"], "scan.png");
    // Bytes handed in beside a part are ignored: the message's own are saved.
    let other = png(4, 4, [9, 9, 9]);
    let got = save(&mut session, json!({"id": id, "src": "cid:scan@example", "cached": b64(&other)}));
    assert_eq!(bytes_of(&got), bytes);
    // A part the HTML does not show is not this command's.
    let (mut session, id) = hold("<p>No picture</p>", vec![part("loose.png", "loose@example", bytes)]);
    assert!(refuse(&mut session, json!({"id": id, "src": "cid:loose@example"})).contains("no such picture"));
}

#[test]
fn a_percent_encoded_part_is_the_one_the_message_names() {
    let bytes = png(3, 3, [4, 5, 6]);
    let (mut session, id) = hold(r#"<img src="cid:a%40b.example">"#, vec![part("a.png", "a@b.example", bytes.clone())]);
    let got = save(&mut session, json!({"id": id, "src": "cid:a%40b.example"}));
    assert_eq!(bytes_of(&got), bytes);
}

#[test]
fn each_kind_of_picture_written_into_the_message_is_saved_as_itself() {
    let pictures = [("png", png(6, 4, [7, 8, 9])), ("jpg", jpeg(6, 4)), ("gif", gif(6, 4)), ("webp", webp(6, 4))];
    let html = pictures.iter().map(|(_, bytes)| format!(r#"<img src="data:image/png;base64,{}">"#, b64(bytes))).collect::<String>();
    let (mut session, id) = hold(&html, Vec::new());
    for (n, (ext, bytes)) in pictures.iter().enumerate() {
        let got = save(&mut session, json!({"id": id, "src": format!("data:{n}"), "width": 640, "dark": false}));
        let name = format!("picture.{ext}");
        assert_eq!((got["name"].as_str(), bytes_of(&got).as_slice()), (Some(name.as_str()), bytes.as_slice()), "{ext}");
    }
    assert!(refuse(&mut session, json!({"id": id, "src": "data:4"})).contains("no such picture"));
    assert!(refuse(&mut session, json!({"id": id, "src": "data:0", "width": 640})).contains("go together"));
    assert!(refuse(&mut session, json!({"id": id, "src": "data:0", "dark": false})).contains("go together"));
}

#[test]
fn the_pane_the_picture_was_read_in_picks_which_one_was_shown() {
    let day = png(8, 8, [240, 240, 240]);
    let night = png(8, 8, [20, 20, 20]);
    let html = format!(
        "<style>.night{{display:none}} @media (prefers-color-scheme: dark){{.night{{display:block !important}} .day{{display:none !important}}}}</style>\
         <img class=day src=\"data:image/png;base64,{}\"><img class=night src=\"data:image/png;base64,{}\">",
        b64(&day),
        b64(&night)
    );
    let (mut session, id) = hold(&html, Vec::new());
    let light = save(&mut session, json!({"id": id, "src": "data:0", "width": 800, "dark": false}));
    let dark = save(&mut session, json!({"id": id, "src": "data:0", "width": 800, "dark": true}));
    assert_eq!(bytes_of(&light), day);
    assert_eq!(bytes_of(&dark), night);
}

#[test]
fn a_picture_on_the_web_is_saved_only_from_bytes_already_kept() {
    let bytes = png(10, 6, [30, 60, 90]);
    let src = "https://img.example/photos/ridge morning.jpg?x=1";
    let html = format!(r#"<img src="{src}" alt="ridge">"#);
    let (mut session, id) = hold(&html, Vec::new());
    let why = refuse(&mut session, json!({"id": id, "src": src}));
    assert!(why.contains("nothing is downloaded"), "{why}");
    let got = save(&mut session, json!({"id": id, "src": src, "cached": b64(&bytes)}));
    assert_eq!((got["name"].as_str(), bytes_of(&got).as_slice()), (Some("ridge morning.png"), bytes.as_slice()));
    // No leaf to name it by.
    let (mut session, id) = hold(r#"<img src="https://img.example/?x=1">"#, Vec::new());
    let got = save(&mut session, json!({"id": id, "src": "https://img.example/?x=1", "cached": b64(&jpeg(5, 5))}));
    assert_eq!(got["name"], "picture.jpg");
    // A tracking picture was never a picture of the message, and bytes offered for it change nothing.
    let (mut session, id) = hold(r#"<p>a</p><img src="https://t.example/o.gif" width="1" height="1"><p>b</p>"#, Vec::new());
    let why = refuse(&mut session, json!({"id": id, "src": "https://t.example/o.gif", "cached": b64(&bytes)}));
    assert!(why.contains("no such picture"), "{why}");
    // What was kept is not a picture.
    let (mut session, id) = hold(r#"<img src="https://img.example/page.png">"#, Vec::new());
    let why = refuse(&mut session, json!({"id": id, "src": "https://img.example/page.png", "cached": b64(b"<html></html>")}));
    assert!(why.contains("not a picture"), "{why}");
    let why = refuse(&mut session, json!({"id": id, "src": "https://img.example/page.png", "cached": "%%%"}));
    assert!(why.contains("base64"), "{why}");
    // A header that names a size, with the pixels after it cut off.
    let torn = header_only(jpeg(8, 8));
    let (mut session, id) = hold(r#"<img src="cid:torn@example">"#, vec![part("torn.jpg", "torn@example", torn)]);
    let why = refuse(&mut session, json!({"id": id, "src": "cid:torn@example"}));
    assert!(why.contains("not a picture"), "{why}");
}

/// The picture's header, through the start of its scan, with the pixels cut away.
fn header_only(mut bytes: Vec<u8>) -> Vec<u8> {
    let at = bytes.windows(2).position(|pair| pair == [0xFF, 0xDA]).unwrap_or(0);
    bytes.truncate(at.saturating_add(8).min(bytes.len().saturating_sub(1)).max(1));
    bytes
}

#[test]
fn a_part_that_is_not_a_picture_is_refused() {
    let (mut session, id) = hold(r#"<img src="cid:note@example" alt="note">"#, vec![part("note.txt", "note@example", b"hello".to_vec())]);
    let why = refuse(&mut session, json!({"id": id, "src": "cid:note@example"}));
    assert!(why.contains("not a picture"), "{why}");
}

#[test]
fn saving_writes_the_bytes_and_leaves_the_message() {
    let bytes = png(5, 5, [1, 1, 1]);
    let (mut session, id) = hold(r#"<img src="cid:shot@example">"#, vec![part("ridge.png", "shot@example", bytes.clone())]);
    let path = std::env::temp_dir().join(format!("mailcraft-save-picture-{}.png", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let got = save(&mut session, json!({"id": id, "src": "cid:shot@example", "path": path.to_str().unwrap()}));
    assert_eq!(got["bytes"], bytes.len());
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    let _ = std::fs::remove_file(&path);
    assert_eq!(session.mailbox.message(id).unwrap().attachments[0].data.as_ref(), &bytes);
    assert!(refuse(&mut session, json!({})).contains("src"));
    assert!(refuse(&mut session, json!({"src": 12})).contains("src"));
    assert!(session.run("mail.savePicture", &json!({"id": "nope", "src": "cid:shot@example"})).is_err());
}
