// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A reply or forward quotes a message as the phone's reading view showed it: its HTML, or its
//! text part when the view showed that instead. Invented people and `example.org` addresses only.

use std::sync::Arc;

use mailcraft_engine::Session;
use mailcraft_model::{Address, Attachment, FolderKind, Message};

use crate::fake_tests::{FakePlatform, block_on, now};
use crate::{ComposeStart, Core};

const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";

/// HTML that shows a picture, or nothing at all, beside a text part that says something else.
fn shapes() -> Vec<String> {
    let data = format!("data:image/png;base64,{}", mailcraft_charset::base64::STANDARD.encode(PNG));
    [
        "<img src=\"cid:logo%40news.example\">".to_string(),
        "<img src=\"cid:<logo@news.example>\">".to_string(),
        "<img src=\"cid:missing@news.example\">".to_string(),
        "<img src=\"https://pics.example/a.png\" alt=\"ALTWORDS\">".to_string(),
        format!("<img src=\"{data}\">"),
        "<script>x()</script>".to_string(),
    ]
    .into()
}

fn core(html: &str) -> (Arc<Core>, u64) {
    let html = html.to_string();
    let core = Core::start(FakePlatform::new(), move || {
        let mut mb = mailcraft_model::Mailbox::with_local_account("Avery", "avery@example.org");
        let account = mb.accounts[0].id;
        let inbox = mb.folder_of_kind(account, FolderKind::Inbox).unwrap();
        let mut m = Message { account, folder: inbox, subject: "Logo".into(), ..Message::default() };
        m.from = Address::new("Ada Quill", "ada@example.org");
        m.body = mailcraft_model::Body { text: "SENDER ALTERNATIVE".into(), html: Some(html) };
        m.attachments =
            vec![Attachment { content_id: "logo@news.example".into(), inline: true, ..Attachment::new("logo.png", "image/png", PNG.to_vec()) }];
        mb.add_message(m).unwrap();
        Session::at(mb, now(), 0)
    })
    .unwrap();
    let id = block_on(core.call(|s| Ok(s.mailbox.messages.keys().copied().max().unwrap_or(0)))).unwrap();
    (core, id)
}

#[test]
fn a_reply_quotes_what_the_reading_view_showed() {
    for html in shapes() {
        for how in [ComposeStart::Reply { id: 0 }, ComposeStart::Forward { id: 0 }] {
            let (core, id) = core(&html);
            let view = block_on(core.message(id, 400.0, false)).unwrap();
            let how = match how {
                ComposeStart::Reply { .. } => ComposeStart::Reply { id },
                _ => ComposeStart::Forward { id },
            };
            block_on(core.compose_start(how)).unwrap();
            let (as_text, quoted_text, quote) = block_on(core.call(move |s| {
                let c = s.compose.as_ref().unwrap();
                Ok((s.read_as(id, mailcraft_engine::ReadIn::Pane).map(|r| r.1), c.body.plain().contains("SENDER ALTERNATIVE"), c.quote.clone()))
            }))
            .unwrap();
            // The view shows the HTML whenever it holds a picture; the text part only for HTML of nothing.
            let shown_as_text = html.starts_with("<script");
            assert_eq!(as_text, Some(shown_as_text), "{html}");
            assert!(!view.blocks.is_empty(), "{html}");
            assert_eq!(quoted_text, shown_as_text, "{html}");
            assert_eq!(quote.is_none(), shown_as_text, "{html}");
            assert!(quote.is_none_or(|q| !q.text.contains("SENDER ALTERNATIVE") && !q.html.contains("SENDER ALTERNATIVE")), "{html}");
        }
    }
}

/// A message read before its body came (no HTML yet) is not taken for one shown as its text
/// part: the HTML that comes later is quoted, not the text beside it.
#[test]
fn a_reply_after_the_body_came_quotes_its_html_not_the_text_read_before() {
    let (core, id) = core("<p>Shown words</p>");
    block_on(core.call(move |s| {
        s.mailbox.message_mut(id).unwrap().body = mailcraft_model::Body { text: "Preview words".into(), html: None };
        Ok(())
    }))
    .unwrap();
    block_on(core.message(id, 400.0, false)).unwrap();
    block_on(core.call(move |s| {
        assert_eq!(s.read_as(id, mailcraft_engine::ReadIn::Pane).map(|r| r.1), Some(false));
        s.mailbox.message_mut(id).unwrap().body =
            mailcraft_model::Body { text: "SENDER ALTERNATIVE".into(), html: Some("<p>Shown words</p>".into()) };
        Ok(())
    }))
    .unwrap();
    block_on(core.compose_start(ComposeStart::Reply { id })).unwrap();
    let (body, quote) = block_on(core.call(|s| {
        let c = s.compose.as_ref().unwrap();
        Ok((c.body.plain(), c.quote.clone()))
    }))
    .unwrap();
    assert!(!body.contains("SENDER ALTERNATIVE"), "{body}");
    assert!(quote.is_some_and(|q| q.html.contains("Shown words")));
}
