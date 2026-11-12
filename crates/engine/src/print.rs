// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Messages as a PDF to print or keep (#32): each under a header with its subject, sender,
//! recipients, date and attachments, its body as the reading pane shows it, in its light colours.
//! The text is drawn in the bundled faces, with the system's faces for other scripts behind them
//! ([`Session::print_fallbacks`]); pictures attached to the message or written into it are printed,
//! and nothing is fetched.

use std::sync::Arc;

use mailcraft_html::{BlockKind, Env};
use mailcraft_model::{Id, Message};
use mailcraft_print::{Doc, Face, Options, Paper};

use crate::cmd::fail;
use crate::{CmdError, Session};

/// The most messages one print takes.
pub const MAX_MESSAGES: usize = 100;
/// How wide a body is laid out for its `@media` rules, in CSS pixels: a printed page's.
const PAGE_PX: f32 = 650.0;

/// The words of a print, in the interface language: field, key in params, English.
macro_rules! labels {
    ($($field:ident $key:literal $english:literal,)*) => {
        /// The words of a printed header, page foot and table heads, in the interface language.
        #[derive(Clone, Debug)]
        pub struct Labels {
            $(pub $field: String,)*
        }

        impl Default for Labels {
            fn default() -> Labels {
                Labels { $($field: $english.into(),)* }
            }
        }

        impl Labels {
            /// The keys params name them by, with the English words.
            pub const KEYS: &[(&str, &str)] = &[$(($key, $english),)*];

            /// The labels params give (`{"from": …, "page": …}`), each at most 80 characters,
            /// the rest English.
            pub fn from_params(v: Option<&serde_json::Value>) -> Labels {
                let mut l = Labels::default();
                let Some(v) = v else { return l };
                let take = |key: &str, slot: &mut String| {
                    if let Some(text) = v.get(key).and_then(|t| t.as_str()).map(str::trim).filter(|t| !t.is_empty()) {
                        *slot = text.chars().filter(|c| !c.is_control()).take(80).collect();
                    }
                };
                $(take($key, &mut l.$field);)*
                l
            }

            /// The labels as params.
            pub fn to_params(&self) -> serde_json::Value {
                let mut map = serde_json::Map::new();
                $(map.insert($key.into(), serde_json::Value::String(self.$field.clone()));)*
                serde_json::Value::Object(map)
            }
        }
    };
}

labels! {
    from "from" "From:",
    sent "sent" "Sent:",
    to "to" "To:",
    cc "cc" "Cc:",
    attachments "attachments" "Attachments:",
    page "page" "Page {n} of {m}",
    calendar "calendar" "Calendar",
    contacts "contacts" "Contacts",
    tasks "tasks" "Tasks",
    all_day "allDay" "All day",
    nothing "nothing" "Nothing planned",
    subject "subject" "Subject",
    due "due" "Due",
    status "status" "Status",
    importance "importance" "Importance",
    categories "categories" "Categories",
    completed "completed" "Completed",
    not_completed "notCompleted" "Not completed",
    high "high" "High",
    normal "normal" "Normal",
    low "low" "Low",
    email "email" "Email",
    phone "phone" "Phone",
    address "address" "Address",
    birthday "birthday" "Birthday",
    notes "notes" "Notes",
}

/// What a print takes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum What {
    /// Messages: those named, else the selected.
    #[default]
    Messages,
    /// The calendar's days `from`–`to` (default the days in view) in a style (default a month
    /// grid when a month is in view, else each day in turn).
    Calendar { from: Option<mailcraft_time::Date>, to: Option<mailcraft_time::Date>, style: Option<crate::print_views::CalendarStyle> },
    /// Contacts: those named, else those the People list shows.
    Contacts,
    /// Tasks: those named, else those the Tasks list shows.
    Tasks,
}

impl What {
    /// What the module in view prints.
    pub fn of_view(s: &Session) -> What {
        match s.view.module {
            crate::Module::Mail => What::Messages,
            crate::Module::Calendar => What::Calendar { from: None, to: None, style: None },
            crate::Module::People => What::Contacts,
            crate::Module::Tasks => What::Tasks,
        }
    }
}

/// What to print and how.
#[derive(Clone, Debug, Default)]
pub struct Request {
    pub what: What,
    /// The messages, contacts or tasks, in their order; empty for the selected messages, or what
    /// the list shows.
    pub ids: Vec<Id>,
    pub labels: Labels,
    /// `None`: the paper of the owner's region.
    pub paper: Option<Paper>,
}

impl Session {
    /// The messages `ids` names, or the selected ones, at most [`MAX_MESSAGES`].
    fn print_targets(&self, ids: &[Id]) -> Result<Vec<Id>, CmdError> {
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
            if self.mailbox.message(*id).is_none() {
                return Err(failed!("no message with id {id}", id = *id));
            }
        }
        Ok(ids)
    }

    /// The paper of the owner's region.
    pub fn print_paper(&self) -> Paper {
        self.date_region.as_deref().map_or(Paper::A4, Paper::for_region)
    }

    /// What `request` asks for as documents to print.
    pub fn print_docs(&self, request: &Request) -> Result<Vec<Doc>, CmdError> {
        match request.what {
            What::Messages => {}
            What::Calendar { from, to, style } => {
                let (view_from, view_to) = crate::cmd::view::calendar_range(self);
                let from = from.unwrap_or(view_from);
                let to = to.unwrap_or(view_to);
                let style = style.unwrap_or(if self.view.calendar == crate::CalView::Month {
                    crate::print_views::CalendarStyle::Month
                } else {
                    crate::print_views::CalendarStyle::Agenda
                });
                return Ok(vec![self.print_calendar(from, to, style, &request.labels)?]);
            }
            What::Contacts => return Ok(vec![self.print_contacts(&request.ids, &request.labels)?]),
            What::Tasks => return Ok(vec![self.print_tasks(&request.ids, &request.labels)?]),
        }
        let mut docs = Vec::new();
        for id in self.print_targets(&request.ids)? {
            let m = self.loaded_message(id)?;
            docs.push(self.print_doc(&m, &request.labels));
        }
        Ok(docs)
    }

    fn print_doc(&self, m: &Message, labels: &Labels) -> Doc {
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
        let blocks = body_blocks(m);
        let mut pictures = Vec::new();
        for picture in mailcraft_html::images(&blocks) {
            let BlockKind::Image { src, data: None, .. } = &picture.kind else { continue };
            let Some((id, index)) = attachment(m, src) else { continue };
            if pictures.iter().any(|(known, _): &(String, Arc<[u8]>)| known == &id) {
                continue;
            }
            if let Ok(bytes) = self.content(m.id, crate::Part::Attachment(index)) {
                pictures.push((id, Arc::from(bytes.as_slice())));
            }
        }
        Doc { title: m.subject.clone(), fields, blocks, pictures }
    }

    /// The faces to print in: the bundled ones, and the system's for other scripts.
    pub fn print_fonts(&self) -> Result<mailcraft_print::Fonts, CmdError> {
        let mut fallbacks = Vec::new();
        #[cfg(not(target_arch = "wasm32"))]
        for path in self.print_fallbacks.iter().take(mailcraft_print::MAX_FACES.saturating_sub(4)) {
            let Ok(meta) = std::fs::metadata(path) else { continue };
            if meta.len() > mailcraft_print::MAX_FACE_BYTES as u64 {
                continue;
            }
            if let Some(face) = std::fs::read(path).ok().and_then(|bytes| Face::new(bytes, 0)) {
                fallbacks.push(face);
            }
        }
        mailcraft_print::bundled(fallbacks).ok_or_else(|| fail("the font to print in can't be read"))
    }

    /// The messages of `request` as a PDF, with how many pages and messages it has.
    pub fn print_pdf(&self, request: &Request) -> Result<(Vec<u8>, usize, usize), CmdError> {
        let docs = self.print_docs(request)?;
        let fonts = self.print_fonts()?;
        let title = docs.first().map(|d| d.title.clone()).unwrap_or_default();
        let options = Options { paper: request.paper.unwrap_or_else(|| self.print_paper()), title, page_label: request.labels.page.clone() };
        let pages = mailcraft_print::page_text(&docs, &fonts, &options).map_err(|e| fail(e.to_string()))?.len();
        let bytes = mailcraft_print::pdf(&docs, &fonts, &options).map_err(|e| fail(e.to_string()))?;
        Ok((bytes, pages, docs.len()))
    }
}

/// A message's display blocks as the reading pane takes them: from its HTML when that shows
/// something, else from its text.
fn body_blocks(m: &Message) -> Vec<mailcraft_html::Block> {
    let env = Env::for_pane(PAGE_PX, false);
    if let Some(html) = m.body.html.as_deref() {
        let read = mailcraft_html::parse_in(html, &env);
        let shows_picture = mailcraft_html::images(&read.blocks).iter().any(|b| match &b.kind {
            BlockKind::Image { data: Some(_), .. } => true,
            BlockKind::Image { src, .. } => attachment(m, src).is_some(),
            _ => false,
        });
        if mailcraft_html::has_text(&read.blocks) || shows_picture || m.body.text.trim().is_empty() {
            return read.blocks;
        }
    }
    mailcraft_html::text_blocks(&m.body.text)
}

/// The attachment a `cid:` address names, as the reading pane finds it: its id as the address
/// gives it, and its place.
fn attachment(m: &Message, src: &str) -> Option<(String, usize)> {
    let src = src.trim();
    let id = src.get(..4).filter(|scheme| scheme.eq_ignore_ascii_case("cid:")).and_then(|_| src.get(4..))?;
    let id = id.trim().trim_start_matches('<').trim_end_matches('>');
    if id.is_empty() {
        return None;
    }
    let decoded = percent_decode(id);
    let parts = || m.attachments.iter().enumerate().filter(|(_, a)| !a.content_id.is_empty());
    parts()
        .find(|(_, a)| a.content_id == id)
        .or_else(|| parts().find(|(_, a)| a.content_id == decoded))
        .or_else(|| parts().find(|(_, a)| a.content_id.eq_ignore_ascii_case(id) || a.content_id.eq_ignore_ascii_case(&decoded)))
        .map(|(i, _)| (id.to_string(), i))
}

/// `%40` and the like decoded; anything that isn't a valid escape, or doesn't give UTF-8, is left.
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        let escape = (b == b'%').then(|| text.get(i + 1..i + 3)).flatten().and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match escape {
            Some(byte) => {
                out.push(byte);
                i += 3;
            }
            None => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| text.to_string())
}
