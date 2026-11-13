// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Messages as a PDF to print or keep (#32): each under a header with its subject, sender,
//! recipients, date and attachments, its body as the reading pane shows it, in its light colours.
//! The text is drawn in the bundled faces, with the system's faces for other scripts behind them
//! ([`Session::print_fallbacks`]); pictures attached to the message or written into it are printed,
//! and nothing is fetched. A message whose body isn't on this computer yet is refused, not printed
//! from its preview.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use mailcraft_html::{BlockKind, Env};
use mailcraft_model::{Id, Message};
use mailcraft_print::{Doc, Face, Options, Paper};

use crate::cmd::fail;
use crate::{CmdError, Session};

/// The most messages one print takes.
pub const MAX_MESSAGES: usize = 100;
/// How wide a body is laid out for its `@media` rules, in CSS pixels: a printed page's.
const PAGE_PX: f32 = 650.0;
/// The most bytes of attached pictures one message prints, and a whole print.
pub const MAX_MESSAGE_PICTURE_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_PRINT_PICTURE_BYTES: usize = 256 * 1024 * 1024;
/// The most picture addresses of one message that are looked up.
const MAX_PICTURE_ADDRESSES: usize = 4_096;
/// What a message that isn't here yet is refused with.
pub const NOT_HERE: &str = "This message isn't on this computer yet. It will download when you're online.";

/// The words of a printed header and page foot, in the interface language.
#[derive(Clone, Debug)]
pub struct Labels {
    pub from: String,
    pub sent: String,
    pub to: String,
    pub cc: String,
    pub attachments: String,
    /// `{n}` for the page's number, `{m}` for the count.
    pub page: String,
}

impl Default for Labels {
    fn default() -> Labels {
        Labels {
            from: "From:".into(),
            sent: "Sent:".into(),
            to: "To:".into(),
            cc: "Cc:".into(),
            attachments: "Attachments:".into(),
            page: "Page {n} of {m}".into(),
        }
    }
}

impl Labels {
    /// The labels params give (`{"from": …, "page": …}`), each at most 80 characters, the rest
    /// English.
    pub fn from_params(v: Option<&serde_json::Value>) -> Labels {
        let mut l = Labels::default();
        let Some(v) = v else { return l };
        let take = |key: &str, slot: &mut String| {
            if let Some(text) = v.get(key).and_then(|t| t.as_str()).map(str::trim).filter(|t| !t.is_empty()) {
                *slot = text.chars().filter(|c| !c.is_control()).take(80).collect();
            }
        };
        take("from", &mut l.from);
        take("sent", &mut l.sent);
        take("to", &mut l.to);
        take("cc", &mut l.cc);
        take("attachments", &mut l.attachments);
        take("page", &mut l.page);
        l
    }
}

/// What to print and how.
#[derive(Clone, Debug, Default)]
pub struct Request {
    /// The messages, in their order; empty for the selected ones.
    pub ids: Vec<Id>,
    pub labels: Labels,
    /// `None`: the paper of the owner's region.
    pub paper: Option<Paper>,
}

impl Session {
    /// The messages `ids` names, or the selected ones, at most [`MAX_MESSAGES`]: what a print
    /// takes, as it takes them.
    pub fn print_ids(&self, ids: &[Id]) -> Result<Vec<Id>, CmdError> {
        let ids: Vec<Id> = if ids.is_empty() {
            let mut sel = self.sel.messages.clone();
            if sel.is_empty() {
                sel.extend(self.sel.focus());
            }
            sel
        } else {
            ids.to_vec()
        };
        if ids.is_empty() {
            return Err(fail("select a message first"));
        }
        if ids.len() > MAX_MESSAGES {
            return Err(fail(format!("print at most {MAX_MESSAGES} messages at a time")));
        }
        for id in &ids {
            let Some(m) = self.mailbox.message(*id) else { return Err(failed!("no message with id {id}", id = *id)) };
            // Its preview is all that is here: printing that would leave the rest out unsaid.
            if m.unfetched && m.server.is_some() {
                return Err(fail(NOT_HERE));
            }
        }
        Ok(ids)
    }

    /// The paper of the owner's region.
    pub fn print_paper(&self) -> Paper {
        self.date_region.as_deref().map_or(Paper::A4, Paper::for_region)
    }

    /// The messages of `request` as documents to print.
    pub fn print_docs(&self, request: &Request) -> Result<Vec<Doc>, CmdError> {
        let mut docs = Vec::new();
        let mut room = MAX_PRINT_PICTURE_BYTES;
        for id in self.print_ids(&request.ids)? {
            // The body alone: each picture it shows is fetched by itself, once it is known to fit.
            let m = self.loaded_message_where(id, |_| false)?;
            docs.push(print_doc(self, &m, &request.labels, &mut room));
        }
        Ok(docs)
    }
}

impl Session {
    fn print_header(&self, m: &Message, labels: &Labels) -> Vec<(String, String)> {
        let mut fields = Vec::new();
        let from = m.from.full();
        if !from.trim().is_empty() {
            fields.push((labels.from.clone(), from));
        }
        if m.date != 0 {
            let local = self.local(m.date);
            let d = local.date;
            let day = mailcraft_i18n::date::Day { year: d.year, month: d.month, day: d.day, weekday: d.weekday().index() };
            fields.push((labels.sent.clone(), format!("{} {}", mailcraft_i18n::date::long(&self.dated(), day), self.time_said(local))));
        }
        let (to, more) = mailcraft_model::format_list_capped(&m.to, 200);
        if !to.is_empty() {
            fields.push((labels.to.clone(), if more > 0 { format!("{to}; +{more}") } else { to }));
        }
        if !m.cc.is_empty() {
            let (cc, more) = mailcraft_model::format_list_capped(&m.cc, 200);
            fields.push((labels.cc.clone(), if more > 0 { format!("{cc}; +{more}") } else { cc }));
        }
        let files: Vec<&str> = m.attachments.iter().filter(|a| !a.inline).map(|a| a.name.as_str()).take(200).collect();
        if !files.is_empty() {
            fields.push((labels.attachments.clone(), files.join("; ")));
        }
        fields
    }

    /// The faces to print in: the bundled ones, and the system's for other scripts
    /// ([`Session::print_fallbacks`], else [`crate::system_fonts::fallback_paths`]). They are
    /// read and checked once and kept for the next print, while the files are the same.
    pub fn print_fonts(&self) -> Result<mailcraft_print::Fonts, CmdError> {
        static KEPT: Mutex<Option<(Vec<Vec<std::path::PathBuf>>, mailcraft_print::Fonts)>> = Mutex::new(None);
        // A group for each script, its files best first: the host's are one file each.
        let groups: Vec<Vec<std::path::PathBuf>> = match &self.print_fallbacks {
            Some(paths) => paths.iter().map(|p| vec![p.clone()]).collect(),
            None => crate::system_fonts::fallback_paths(),
        };
        if let Ok(kept) = KEPT.lock()
            && let Some((known, fonts)) = kept.as_ref()
            && *known == groups
        {
            return Ok(fonts.clone());
        }
        let fallbacks = fallback_faces(&groups);
        let fonts = mailcraft_print::bundled(fallbacks).ok_or_else(|| fail("the font to print in can't be read"))?;
        if let Ok(mut kept) = KEPT.lock() {
            *kept = Some((groups, fonts.clone()));
        }
        Ok(fonts)
    }

    /// The options a print of `docs` is laid out with.
    pub fn print_options(&self, docs: &[Doc], request: &Request) -> Options {
        let title = docs.first().map(|d| d.title.clone()).unwrap_or_default();
        Options { paper: request.paper.unwrap_or_else(|| self.print_paper()), title, page_label: request.labels.page.clone() }
    }

    /// The messages of `request` as a PDF, with how many pages and messages it has.
    pub fn print_pdf(&self, request: &Request) -> Result<(Vec<u8>, usize, usize), CmdError> {
        let docs = self.print_docs(request)?;
        let fonts = self.print_fonts()?;
        let options = self.print_options(&docs, request);
        let (bytes, pages) = render(&docs, &fonts, &options).map_err(fail)?;
        Ok((bytes, pages, docs.len()))
    }
}

/// The faces of `groups` (a group for each script, its files best first): the first file of each
/// that is a face this prints with, a file an earlier group took covering its group too, at most
/// as many as a print takes beside the bundled faces.
pub(crate) fn fallback_faces(groups: &[Vec<std::path::PathBuf>]) -> Vec<Face> {
    let mut faces: Vec<Face> = Vec::new();
    #[cfg(not(target_arch = "wasm32"))]
    {
        let mut taken: Vec<&std::path::PathBuf> = Vec::new();
        for group in groups {
            if faces.len() >= mailcraft_print::MAX_FACES.saturating_sub(4) {
                break;
            }
            if group.iter().any(|p| taken.contains(&p)) {
                continue;
            }
            for path in group {
                let fits = std::fs::metadata(path).is_ok_and(|m| m.len() <= mailcraft_print::MAX_FACE_BYTES as u64);
                if let Some(face) = fits.then(|| std::fs::read(path).ok()).flatten().and_then(|bytes| Face::new(bytes, 0)) {
                    faces.push(face);
                    taken.push(path);
                    break;
                }
            }
        }
    }
    #[cfg(target_arch = "wasm32")]
    let _ = groups;
    faces
}

/// `docs` laid out once as a PDF, with its page count; what is said when that can't be done.
/// Needs no [`Session`]: the window runs it on a thread of its own.
pub fn render(docs: &[Doc], fonts: &mailcraft_print::Fonts, options: &Options) -> Result<(Vec<u8>, usize), String> {
    mailcraft_print::pdf(docs, fonts, options).map_err(|e| e.to_string())
}

/// Message `m` as a document to print. Each attachment a picture of its body names is taken
/// once, by its place, however the body spells its address; and only when it is a picture the
/// reading pane shows, within the caps for one picture, one message and the print (`room`).
fn print_doc(s: &Session, m: &Message, labels: &Labels, room: &mut usize) -> Doc {
    let fields = s.print_header(m, labels);
    let blocks = body_blocks(m);
    let mut pictures: HashMap<String, Arc<Vec<u8>>> = HashMap::new();
    let mut by_place: HashMap<usize, Option<Arc<Vec<u8>>>> = HashMap::new();
    let mut taken = 0usize;
    for picture in mailcraft_html::images(&blocks).into_iter().take(MAX_PICTURE_ADDRESSES) {
        let BlockKind::Image { src, data: None, .. } = &picture.kind else { continue };
        let src = src.trim();
        if pictures.contains_key(src) {
            continue;
        }
        let Some(index) = mailcraft_model::shown::cid_part(m, src) else { continue };
        let bytes = by_place.entry(index).or_insert_with(|| {
            // Judged by its size and kind before a byte of it is read; one that can't be read is
            // left out, not the print.
            let a = m.attachments.get(index)?;
            let len = a.size();
            let fits = len <= mailcraft_print::MAX_PICTURE_BYTES && taken.saturating_add(len) <= MAX_MESSAGE_PICTURE_BYTES && len <= *room;
            if !fits || !mailcraft_model::shown::is_picture_part(a) {
                return None;
            }
            let data = if a.is_loaded() { a.data.clone() } else { s.content(m.id, crate::Part::Attachment(index)).ok()? };
            if data.len() > len.max(mailcraft_print::MAX_PICTURE_BYTES) || !mailcraft_model::shown::is_picture(&data) {
                return None;
            }
            taken += data.len();
            *room = room.saturating_sub(data.len());
            Some(data)
        });
        if let Some(bytes) = bytes {
            pictures.insert(src.to_string(), bytes.clone());
        }
    }
    Doc { title: m.subject.clone(), fields, blocks, pictures }
}

/// A message's display blocks as the reading pane takes them: from its HTML when that shows
/// something, else from its text.
fn body_blocks(m: &Message) -> Vec<mailcraft_html::Block> {
    let env = Env::for_pane(PAGE_PX, false);
    if let Some(html) = m.body.html.as_deref() {
        let read = mailcraft_html::parse_in(html, &env);
        let shows_picture = mailcraft_html::images(&read.blocks).iter().any(|b| match &b.kind {
            BlockKind::Image { data: Some(_), .. } => true,
            BlockKind::Image { src, .. } => mailcraft_model::shown::cid_part(m, src).is_some(),
            _ => false,
        });
        if mailcraft_html::has_text(&read.blocks) || shows_picture || m.body.text.trim().is_empty() {
            return read.blocks;
        }
    }
    mailcraft_html::text_blocks(&m.body.text)
}
