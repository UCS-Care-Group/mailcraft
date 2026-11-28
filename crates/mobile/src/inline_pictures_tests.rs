// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A picture the HTML shows by `cid:` is part of the body, not an attachment, whatever the sender
//! marked it: the reading view and the list row's paperclip say so as the desktop does. Invented
//! people and `example.org` addresses only.

use std::sync::Arc;

use mailcraft_engine::Session;
use mailcraft_model::{Attachment, FolderKind, Message};

use crate::Core;
use crate::fake_tests::{FakePlatform, block_on, now};

const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";

fn picture(name: &str, content_id: &str) -> Attachment {
    // `inline: false`: as Graph says of a signature picture, or a part marked `attachment`.
    Attachment { content_id: content_id.into(), ..Attachment::new(name, "image/png", PNG.to_vec()) }
}

fn core(html: &str, attachments: Vec<Attachment>) -> (Arc<Core>, u64) {
    let html = html.to_string();
    let core = Core::start(FakePlatform::new(), move || {
        let mut mb = mailcraft_model::Mailbox::with_local_account("Avery", "avery@example.org");
        let account = mb.accounts[0].id;
        let folder = mb.folder_of_kind(account, FolderKind::Inbox).unwrap();
        let mut m = Message { account, folder, subject: "Signature".into(), ..Message::default() };
        m.body = mailcraft_model::Body { text: "Regards".into(), html: Some(html) };
        m.attachments = attachments;
        mb.add_message(m).unwrap();
        Session::at(mb, now(), 0)
    })
    .unwrap();
    let folders = block_on(core.folders(0)).unwrap();
    let inbox = folders.folders.iter().find(|f| f.kind == "inbox").unwrap().id;
    let id = block_on(core.messages(inbox, "all".into(), 0, 5)).unwrap().rows[0].id;
    (core, id)
}

fn row_has_attachments(core: &Core) -> bool {
    let folders = block_on(core.folders(0)).unwrap();
    let inbox = folders.folders.iter().find(|f| f.kind == "inbox").unwrap().id;
    block_on(core.messages(inbox, "all".into(), 0, 5)).unwrap().rows[0].has_attachments
}

#[test]
fn signature_pictures_the_html_shows_are_not_listed_nor_give_a_paperclip() {
    let html = "<p>Regards</p><img src=\"cid:image001.png@01DB0000.0000AAAA\"><img src=\"CID:<IMAGE002.PNG@01db0000.0000aaaa>\">";
    let (core, id) =
        core(html, vec![picture("image001.png", "image001.png@01DB0000.0000AAAA"), picture("image002.png", "image002.png@01DB0000.0000AAAA")]);
    let view = block_on(core.message(id, 400.0, false)).unwrap();
    assert_eq!(view.attachments.len(), 2);
    assert!(view.attachments.iter().all(|a| a.inline), "{:?}", view.attachments);
    assert!(!row_has_attachments(&core));
}

#[test]
fn a_picture_the_html_does_not_show_is_listed() {
    let html = "<p>Regards</p><img src=\"cid:image001.png@01DB0000.0000AAAA\">";
    let (core, id) = core(html, vec![picture("image001.png", "image001.png@01DB0000.0000AAAA"), picture("photo.png", "photo.png@01DB0000.0000AAAA")]);
    let view = block_on(core.message(id, 400.0, false)).unwrap();
    assert_eq!(view.attachments.iter().map(|a| (a.name.as_str(), a.inline)).collect::<Vec<_>>(), [("image001.png", true), ("photo.png", false)]);
    assert!(row_has_attachments(&core));
}

#[test]
fn a_picture_asked_for_with_an_escaped_content_id_is_drawn_where_it_is_not_listed() {
    let (core, id) = core("<p>Regards</p><img src=\"cid:logo%40a.example\">", vec![picture("logo.png", "logo@a.example")]);
    let view = block_on(core.message(id, 400.0, false)).unwrap();
    assert!(view.attachments[0].inline);
    // The body draws it: its block names the part.
    let blocks = format!("{:?}", view.blocks);
    assert!(blocks.contains("picture: Some(\"cid:logo%40a.example\")"), "{blocks}");
}
