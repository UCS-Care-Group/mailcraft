// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! One message with its display blocks, and the bytes of a picture it names. Nothing here
//! changes the mailbox (opening a message does not mark it read: `mail.markRead` does), and
//! nothing is fetched from the web.

use mailcraft_engine::{Part, Session};
use mailcraft_html::Env;
use mailcraft_model::{Attachment, FlagStatus, Id, Message};

use crate::blocks::{self, Pictures};
use crate::records::{AttachmentRecord, MessageView, PictureBytes};
use crate::text::{self, MAX_CATEGORIES, MAX_LISTED, MAX_NAME, MAX_PEOPLE_IN_ITEM, MAX_SUBJECT};
use crate::{Core, CoreError};

/// The most bytes of a picture handed over.
const MAX_PICTURE_BYTES: usize = 20 * 1024 * 1024;

fn bad(message: &str) -> CoreError {
    CoreError::BadRequest { reason: message.to_string() }
}

fn failed(message: &str) -> CoreError {
    CoreError::Failed { reason: message.to_string() }
}

/// The attachment a `cid:` address names: by its content id, as the reading pane finds it.
fn attachment_of<'a>(message: &'a Message, src: &str) -> Option<(usize, &'a Attachment)> {
    let id = src.trim().get(4..).filter(|_| src.trim().get(..4).is_some_and(|s| s.eq_ignore_ascii_case("cid:")))?;
    let id = id.trim().trim_start_matches('<').trim_end_matches('>');
    let parts = || message.attachments.iter().enumerate().filter(|(_, a)| !a.content_id.is_empty());
    parts().find(|(_, a)| a.content_id == id).or_else(|| parts().find(|(_, a)| a.content_id.eq_ignore_ascii_case(id)))
}

/// A picture's kind, told from its own bytes.
fn sniff(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.get(..4) == Some(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        Some("image/webp")
    } else {
        None
    }
}

/// The message's blocks for a pane: its HTML when it reads to something, else its text.
fn read(message: &Message, env: &Env) -> (Vec<mailcraft_html::Block>, bool) {
    let read = message.body.html.as_deref().map(|html| mailcraft_html::parse_in(html, env));
    match read {
        Some(r) if mailcraft_html::has_text(&r.blocks) || !mailcraft_html::images(&r.blocks).is_empty() || message.body.text.trim().is_empty() => {
            (r.blocks, r.dark_design)
        }
        _ => (mailcraft_html::text_blocks(&message.body.text), false),
    }
}

fn pane(width: f32, dark: bool) -> (Env, u16) {
    let env = Env::for_pane(width, dark);
    (env, env.width())
}

fn load(s: &Session, id: Id) -> Result<std::sync::Arc<Message>, CoreError> {
    // The body alone: an attachment's bytes (or a signed original) are not read to show a message.
    s.loaded_message_where(id, |_| false).map_err(|_| failed(s.interface_language.t("the message could not be read")))
}

#[uniffi::export]
impl Core {
    /// Message `id` as display blocks, for a body `width` points wide in a dark window or a
    /// light one: the message's `@media` rules are tested against them.
    pub async fn message(&self, id: u64, width: f32, dark: bool) -> Result<MessageView, CoreError> {
        let (env, width) = pane(width, dark);
        self.call(move |s| {
            let m = load(s, id)?;
            let (blocks, dark_design) = read(&m, &env);
            let has_part = |src: &str| attachment_of(&m, src).is_some();
            let views = blocks::views(&blocks, &Pictures { has_part: &has_part, width, dark });
            let remote_pictures = mailcraft_html::images(&blocks)
                .into_iter()
                .filter(|b| matches!(&b.kind, mailcraft_html::BlockKind::Image { src, .. } if !mailcraft_html::is_embedded(src)))
                .count();
            let people = |a: &[mailcraft_model::Address]| text::first(a, MAX_PEOPLE_IN_ITEM, crate::queries::address);
            Ok(MessageView {
                rev: s.rev(),
                id: m.id,
                account: m.account,
                folder: m.folder,
                from: crate::queries::address(&m.from),
                to: people(&m.to),
                cc: people(&m.cc),
                subject: text::clip(&m.subject, MAX_SUBJECT),
                when: mailcraft_engine::list::when(&m),
                unread: !m.read,
                flagged: m.flag == FlagStatus::Flagged,
                categories: text::first(&m.categories, MAX_CATEGORIES, |c| text::clip(c, MAX_NAME)),
                attachments: text::first(m.attachments.iter().enumerate(), MAX_LISTED, |(i, a)| AttachmentRecord {
                    index: text::count_of(i),
                    name: text::clip(&a.name, MAX_NAME),
                    mime: text::clip(&a.mime, MAX_NAME),
                    size: u64::try_from(a.size()).unwrap_or(u64::MAX),
                    inline: a.inline,
                }),
                blocks: views,
                dark_design,
                remote_pictures: text::count_of(remote_pictures),
            })
        })
        .await
    }

    /// The bytes of a picture of message `id` that one of its blocks names (`picture` of an
    /// image block): a part of the message, or one written into it. Only a PNG, JPEG, GIF or
    /// WebP, by its own bytes, within 20 MB, is handed over.
    pub async fn picture(&self, id: u64, reference: String) -> Result<PictureBytes, CoreError> {
        if reference.len() > text::MAX_URL {
            return Err(bad("that is not a picture of the message"));
        }
        self.call_holding(reference.len(), move |s| {
            let m = load(s, id)?;
            let bytes: Vec<u8> = if let Some(rest) = reference.strip_prefix("data:") {
                let mut parts = rest.split(':');
                let n: usize = parts.next().and_then(|n| n.parse().ok()).ok_or_else(|| bad("that is not a picture of the message"))?;
                let width: f32 =
                    parts.next().and_then(|n| n.parse::<u16>().ok()).map(f32::from).ok_or_else(|| bad("that is not a picture of the message"))?;
                let dark = parts.next() == Some("1");
                let (blocks, _) = read(&m, &Env::for_pane(width, dark));
                let picture = mailcraft_html::images(&blocks)
                    .into_iter()
                    .filter_map(|b| match &b.kind {
                        mailcraft_html::BlockKind::Image { data: Some(data), .. } => Some(data),
                        _ => None,
                    })
                    .nth(n)
                    .ok_or_else(|| bad("the message has no such picture"))?;
                picture.to_vec()
            } else {
                let (index, a) = attachment_of(&m, &reference).ok_or_else(|| bad("the message has no such picture"))?;
                if a.size() > MAX_PICTURE_BYTES {
                    return Err(failed(s.interface_language.t("the picture is too large")));
                }
                s.content(id, Part::Attachment(index)).map_err(|_| failed(s.interface_language.t("the picture could not be read")))?.to_vec()
            };
            if bytes.len() > MAX_PICTURE_BYTES {
                return Err(failed(s.interface_language.t("the picture is too large")));
            }
            let mime = sniff(&bytes).ok_or_else(|| failed(s.interface_language.t("that is not a picture that can be shown")))?;
            Ok(PictureBytes { rev: s.rev(), mime: mime.to_string(), bytes })
        })
        .await
    }
}
