// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The stand-in's drafts and attachments, as Graph v1.0 documents them ("Create message",
//! "message: createReply", "createReplyAll", "createForward", "Update message", "Add attachment",
//! "attachment: createUploadSession", "Upload large attachments", "Delete attachment"):
//!
//! - `POST /v1.0/me/messages` makes a draft in Drafts, from JSON (`subject`, `body`, the three
//!   lists of recipients, `importance`, `categories`, `singleValueExtendedProperties` and
//!   `internetMessageHeaders`, whose names have to begin with `x-`: anything else is 400) or from
//!   MIME (`Content-Type: text/plain`, the message in base64). Answered 201 with the draft.
//! - `POST /v1.0/me/messages/{id}/createReply`, `/createReplyAll` and `/createForward` make a
//!   draft answering (or forwarding) the message: in its conversation, `RE:` or `FW:` and its
//!   subject, addressed as an answer is, `In-Reply-To` it (an answer), its attachments with it
//!   (a forward). The body may say a `comment` or a `message` of writable properties, not a
//!   comment and a body both (400).
//! - `PATCH` of a draft may change its subject, body and recipients too
//!   ([`super::changes`]); of anything else, not (400). `internetMessageHeaders` are written when
//!   a draft is made, and never after.
//! - **Attachments**: `GET`/`POST /me/messages/{id}/attachments` (a `fileAttachment` of less than
//!   3 MB, its bytes in base64; more is 413), `GET`/`DELETE …/attachments/{id}`, and
//!   `POST …/attachments/createUploadSession` for one of 3 MB to 150 MB (less is 400, as Graph's
//!   `ErrorAttachmentSizeShouldNotBeLessThanMinimumSize`), whose `uploadUrl` takes the bytes in
//!   `PUT`s of at most 4 MB, each saying its `Content-Range`, in order, and **no**
//!   `Authorization` (401: the address is its own permission). The last answers 201 with a
//!   `Location` and no body. An attachment's `size` is larger than what it holds, as Graph's is.
//! - **Answers lost on demand** ([`State::lose_next`](super::State::lose_next)): a request of a
//!   method and an address (`*` for any one piece of it) is carried out and its answer lost.

use mailcraft_charset::base64::STANDARD;
use serde_json::{Map, Value, json};

use super::messages::Message;
use super::{Answer, Asked, Refusal, State, bad, not_found, refuse};

/// Most bytes of an attachment added in one request, and the least and most an upload session
/// takes; most bytes of one piece of an upload.
pub(super) const DIRECT_MOST: usize = 3 * 1024 * 1024;
const SESSION_LEAST: u64 = 3 * 1024 * 1024;
const SESSION_MOST: u64 = 150 * 1024 * 1024;
const PIECE_MOST: usize = 4 * 1024 * 1024;
/// What Graph adds to an attachment's size beyond what it holds.
const SIZE_OVERHEAD: usize = 213;
/// Longest subject, body and list of recipients taken.
const MAX_SUBJECT: usize = 255 * 4;
const MAX_RECIPIENTS: usize = 500;

/// An attachment of a message of the stand-in.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Attached {
    pub id: String,
    pub name: String,
    pub content_type: String,
    pub bytes: Vec<u8>,
    pub inline: bool,
    pub content_id: String,
}

impl Attached {
    fn json(&self, with_bytes: bool) -> Value {
        let mut out = json!({
            "@odata.type": "#microsoft.graph.fileAttachment",
            "id": self.id,
            "name": self.name,
            "contentType": self.content_type,
            "size": self.bytes.len() + SIZE_OVERHEAD,
            "isInline": self.inline,
            "contentId": if self.content_id.is_empty() { Value::Null } else { json!(self.content_id) },
        });
        if with_bytes {
            out["contentBytes"] = json!(STANDARD.encode(&self.bytes));
        }
        out
    }
}

/// An upload of a large attachment under way.
#[derive(Clone, Debug, Default)]
pub(super) struct Upload {
    message: String,
    attached: Attached,
    size: u64,
}

/// What a draft is made with, or changed by: each part it names.
#[derive(Debug, Default)]
pub(super) struct Written {
    pub subject: Option<String>,
    pub body: Option<(bool, String)>,
    pub to: Option<Vec<String>>,
    pub cc: Option<Vec<String>>,
    pub bcc: Option<Vec<String>>,
    pub reply_to: Option<Vec<String>>,
}

impl Written {
    pub(super) fn any(&self) -> bool {
        self.subject.is_some() || self.body.is_some() || self.to.is_some() || self.cc.is_some() || self.bcc.is_some() || self.reply_to.is_some()
    }

    /// Take `name` when it is one of a draft's own parts: whether it was.
    pub(super) fn take(&mut self, name: &str, value: &Value) -> Result<bool, Refusal> {
        match name {
            "subject" => {
                let subject = value.as_str().filter(|s| s.len() <= MAX_SUBJECT).ok_or_else(|| bad("'subject' is text."))?;
                self.subject = Some(subject.to_string());
            }
            "body" => self.body = Some(body_of(value)?),
            "toRecipients" => self.to = Some(recipients(value, name)?),
            "ccRecipients" => self.cc = Some(recipients(value, name)?),
            "bccRecipients" => self.bcc = Some(recipients(value, name)?),
            "replyTo" => self.reply_to = Some(recipients(value, name)?),
            _ => return Ok(false),
        }
        Ok(true)
    }

    pub(super) fn apply(self, m: &mut Message) {
        if let Some(subject) = self.subject {
            m.subject = subject;
        }
        if let Some((html, content)) = self.body {
            m.preview = preview_of(&content, html);
            (m.html, m.body) = (html, content);
        }
        if let Some(to) = self.to {
            m.to = to;
        }
        if let Some(cc) = self.cc {
            m.cc = cc;
        }
        if let Some(bcc) = self.bcc {
            m.bcc = bcc;
        }
        if let Some(reply_to) = self.reply_to {
            m.reply_to = reply_to;
        }
    }
}

/// A body as Graph writes one: `{"contentType": "html" | "text", "content"}`.
fn body_of(value: &Value) -> Result<(bool, String), Refusal> {
    let wrong = || bad("'body' is an itemBody: a contentType (text or html) and its content.");
    let map = value.as_object().ok_or_else(wrong)?;
    if map.keys().any(|key| !matches!(key.as_str(), "contentType" | "content" | "@odata.type")) {
        return Err(wrong());
    }
    let html = match map.get("contentType").and_then(Value::as_str).map(str::to_ascii_lowercase).as_deref() {
        Some("html") => true,
        Some("text") | None => false,
        Some(_) => return Err(wrong()),
    };
    let content = map.get("content").and_then(Value::as_str).ok_or_else(wrong)?;
    Ok((html, content.to_string()))
}

/// A list of recipients: `[{"emailAddress": {"address", "name"?}}]`, the addresses.
fn recipients(value: &Value, name: &str) -> Result<Vec<String>, Refusal> {
    let wrong = || bad(format!("'{name}' is a list of recipients, each with an emailAddress that has an address."));
    let list = value.as_array().filter(|list| list.len() <= MAX_RECIPIENTS).ok_or_else(wrong)?;
    list.iter()
        .map(|one| {
            let address = one.get("emailAddress").and_then(|e| e.get("address")).and_then(Value::as_str).ok_or_else(wrong)?;
            if address.contains('@') && !address.chars().any(|c| c.is_control() || c.is_whitespace()) {
                Ok(address.to_string())
            } else {
                Err(refuse(400, "ErrorInvalidRecipients", "At least one recipient isn't valid."))
            }
        })
        .collect()
}

/// The first words of a body, as its preview.
fn preview_of(content: &str, html: bool) -> String {
    let text = if html {
        let (mut out, mut tag) = (String::new(), false);
        for c in content.chars() {
            match c {
                '<' => tag = true,
                '>' => tag = false,
                c if !tag => out.push(c),
                _ => {}
            }
        }
        out
    } else {
        content.to_string()
    };
    text.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(255).collect()
}

/// A draft's MIME, made of what it has.
pub(super) fn mime_of(m: &Message) -> String {
    let date = mailcraft_time::DateTime::from_timestamp(m.received, 0);
    let mut head = format!("From: {}\r\nTo: {}\r\n", m.from, m.to.join(", "));
    if !m.cc.is_empty() {
        head.push_str(&format!("Cc: {}\r\n", m.cc.join(", ")));
    }
    if !m.bcc.is_empty() {
        head.push_str(&format!("Bcc: {}\r\n", m.bcc.join(", ")));
    }
    if !m.reply_to.is_empty() {
        head.push_str(&format!("Reply-To: {}\r\n", m.reply_to.join(", ")));
    }
    head.push_str(&format!("Subject: {}\r\nDate: {}\r\nMessage-ID: {}\r\n", m.subject, date.iso(), m.internet_id));
    if !m.in_reply_to.is_empty() {
        head.push_str(&format!("In-Reply-To: {}\r\n", m.in_reply_to));
    }
    head.push_str("MIME-Version: 1.0\r\n");
    let kind = if m.html { "text/html" } else { "text/plain" };
    let text = m.text();
    if m.attachments.is_empty() {
        return format!("{head}Content-Type: {kind}; charset=utf-8\r\n\r\n{text}\r\n");
    }
    let boundary = "=_standin_boundary";
    let mut out = format!(
        "{head}Content-Type: multipart/mixed; boundary=\"{boundary}\"\r\n\r\n--{boundary}\r\nContent-Type: {kind}; charset=utf-8\r\n\r\n{text}\r\n"
    );
    for a in &m.attachments {
        let disposition = if a.inline { "inline" } else { "attachment" };
        out.push_str(&format!(
            "--{boundary}\r\nContent-Type: {}; name=\"{}\"\r\nContent-Disposition: {disposition}; filename=\"{}\"\r\nContent-Transfer-Encoding: base64\r\n",
            a.content_type, a.name, a.name
        ));
        if !a.content_id.is_empty() {
            out.push_str(&format!("Content-ID: <{}>\r\n", a.content_id));
        }
        out.push_str("\r\n");
        let encoded = STANDARD.encode(&a.bytes);
        for line in encoded.as_bytes().chunks(76) {
            out.push_str(&String::from_utf8_lossy(line));
            out.push_str("\r\n");
        }
    }
    out.push_str(&format!("--{boundary}--\r\n"));
    out
}

/// Whether `pattern` (`POST /v1.0/me/messages/*/send`) names the request `method path`.
pub(super) fn aims_at(pattern: &str, method: &str, path: &[String]) -> bool {
    let Some((want, rest)) = pattern.split_once(' ') else { return false };
    let pieces: Vec<&str> = rest.split('/').filter(|p| !p.is_empty()).collect();
    want == method && pieces.len() == path.len() && pieces.iter().zip(path).all(|(p, have)| *p == "*" || p == have)
}

impl State {
    fn new_internet_id(&mut self) -> String {
        let (_, n) = self.mailbox.new_id();
        format!("<draft{n}@{}>", self.owner.split('@').nth(1).unwrap_or("example.org"))
    }

    /// A new draft in Drafts, as the owner's: its id.
    fn new_draft(&mut self, how: impl FnOnce(&mut Message)) -> String {
        let m = self.draft_of(how);
        self.mailbox.put(m)
    }

    /// A draft in Drafts, as the owner's, not yet the stand-in's.
    fn draft_of(&mut self, how: impl FnOnce(&mut Message)) -> Message {
        let drafts = self.well_known("drafts");
        let internet_id = self.new_internet_id();
        let mut m = Message {
            folder: drafts,
            received: self.clock,
            from: self.owner.clone(),
            read: true,
            flag: "notFlagged".into(),
            importance: "normal".into(),
            internet_id,
            draft: true,
            ..Default::default()
        };
        how(&mut m);
        m
    }

    /// `POST /me/messages`: a draft from JSON or from MIME.
    pub(super) fn create_message(&mut self, asked: &Asked) -> Result<Answer, Refusal> {
        let look = State::look_of(asked, false)?;
        let mime = asked.header("content-type").to_ascii_lowercase().starts_with("text/plain");
        let m = if mime {
            let raw = STANDARD
                .decode(asked.body.trim().as_bytes())
                .map_err(|_| refuse(400, "ErrorMimeContentInvalidBase64String", "The MIME content is not valid base64."))?;
            self.message_of_mime(&String::from_utf8_lossy(&raw))?
        } else {
            self.message_of_json(&asked.json())?
        };
        let id = self.mailbox.put(m);
        Ok((201, Some(self.message_json(&id, &look))))
    }

    /// A new draft as a message is written in JSON, not yet the stand-in's.
    pub(super) fn message_of_json(&mut self, body: &Value) -> Result<Message, Refusal> {
        let map = body.as_object().ok_or_else(|| bad("A message is a JSON object."))?;
        let mut written = Written::default();
        let (mut importance, mut categories, mut extended, mut headers) = (None, None, Vec::new(), Vec::new());
        for (name, value) in map {
            if written.take(name, value)? {
                continue;
            }
            match name.as_str() {
                "importance" => match value.as_str() {
                    Some(level @ ("low" | "normal" | "high")) => importance = Some(level.to_string()),
                    _ => return Err(bad("'importance' is low, normal or high.")),
                },
                "categories" => {
                    let names: Option<Vec<String>> = value.as_array().map(|all| all.iter().filter_map(Value::as_str).map(str::to_string).collect());
                    categories = Some(names.ok_or_else(|| bad("'categories' is a list of names."))?);
                }
                "singleValueExtendedProperties" => extended = super::changes::extended_of(value)?,
                "internetMessageHeaders" => headers = headers_of(value)?,
                "@odata.type" => {}
                other => {
                    return Err(refuse(400, "RequestBodyRead", format!("The property '{other}' is not one a new message is made with here.")));
                }
            }
        }
        Ok(self.draft_of(|m| {
            written.apply(m);
            if let Some(importance) = importance {
                m.importance = importance;
            }
            m.categories = categories.unwrap_or_default();
            m.extended = extended;
            m.headers = headers;
        }))
    }

    /// `POST /me/mailFolders/{id}/messages`: a message made in that folder from JSON (#70). A
    /// draft unless `PidTagMessageFlags` written with it say it isn't unsent; what a message that
    /// was received has besides a draft's parts (its sender, read state, flag, when it was
    /// received, its `Message-ID`) as written.
    pub(super) fn create_in_folder(&mut self, asked: &Asked, named: &str) -> Result<Answer, Refusal> {
        let folder = self.mail_folder(named)?.id.clone();
        let look = State::look_of(asked, false)?;
        let mut body = asked.json();
        let map = body.as_object_mut().ok_or_else(|| bad("A message is a JSON object."))?;
        let address = |value: Option<Value>, name: &str| -> Result<Option<String>, Refusal> {
            match value {
                None => Ok(None),
                Some(value) => value
                    .pointer("/emailAddress/address")
                    .and_then(Value::as_str)
                    .map(|a| Some(a.to_string()))
                    .ok_or_else(|| bad(format!("'{name}' is a recipient."))),
            }
        };
        let from = address(map.remove("from"), "from")?;
        address(map.remove("sender"), "sender")?;
        let reply_to = map.remove("replyTo");
        if reply_to.is_some() {
            recipients(reply_to.as_ref().unwrap_or(&Value::Null), "replyTo")?;
        }
        let read = match map.remove("isRead") {
            None => None,
            Some(value) => Some(value.as_bool().ok_or_else(|| bad("'isRead' is true or false."))?),
        };
        let flag = match map.remove("flag") {
            None => None,
            Some(value) => match value.get("flagStatus").and_then(Value::as_str) {
                Some(status @ ("notFlagged" | "flagged" | "complete")) => Some(status.to_string()),
                _ => return Err(bad("'flagStatus' is notFlagged, flagged or complete.")),
            },
        };
        let mut m = self.message_of_json(&body)?;
        m.folder = folder;
        let property = |m: &Message, id: &str| m.extended.iter().find(|(have, _)| have.eq_ignore_ascii_case(id)).map(|(_, v)| v.clone());
        if !self.drafts_only
            && let Some(flags) = property(&m, "Integer 0x0E07")
        {
            let flags: i64 = flags.trim().parse().map_err(|_| bad("PidTagMessageFlags is a number."))?;
            m.draft = flags & 0x8 != 0;
            m.read = flags & 0x1 != 0;
        }
        if let Some(read) = read {
            m.read = read;
        }
        if let Some(from) = from {
            m.from = from;
        }
        if let Some(flag) = flag {
            m.flag = flag;
        }
        if let Some(received) = property(&m, "SystemTime 0x0E06") {
            let wall = super::wall_of(received.trim_end_matches('Z')).ok_or_else(|| bad("A SystemTime is a date and a time."))?;
            m.received = super::instant_in(wall, "UTC").ok_or_else(|| bad("A SystemTime is a date and a time."))?;
        }
        if let Some(id) = property(&m, "String 0x1035") {
            m.internet_id = id;
        }
        let id = self.mailbox.put(m);
        Ok((201, Some(self.message_json(&id, &look))))
    }

    /// A new draft made of its MIME, not yet the stand-in's.
    pub(super) fn message_of_mime(&mut self, text: &str) -> Result<Message, Refusal> {
        let wrong = || refuse(400, "ErrorMimeContentInvalidBase64String", "The MIME content is not valid base64.");
        let (head, body) = text.split_once("\r\n\r\n").or_else(|| text.split_once("\n\n")).ok_or_else(wrong)?;
        let field = |name: &str| {
            head.lines().find_map(|line| {
                let (have, value) = line.split_once(':')?;
                have.trim().eq_ignore_ascii_case(name).then(|| value.trim().to_string())
            })
        };
        let list = |name: &str| {
            field(name)
                .map(|v| v.split(',').map(|a| a.trim().trim_start_matches('<').trim_end_matches('>').to_string()).collect())
                .unwrap_or_default()
        };
        let html = field("Content-Type").is_some_and(|t| t.to_ascii_lowercase().starts_with("text/html"));
        let (subject, to, cc, bcc) = (field("Subject").unwrap_or_default(), list("To"), list("Cc"), list("Bcc"));
        let (in_reply_to, internet_id) = (field("In-Reply-To").unwrap_or_default(), field("Message-ID"));
        let body = body.trim_end().to_string();
        Ok(self.draft_of(|m| {
            m.subject = subject;
            (m.to, m.cc, m.bcc) = (to, cc, bcc);
            m.preview = preview_of(&body, html);
            (m.html, m.body, m.in_reply_to) = (html, body, in_reply_to);
            if let Some(id) = internet_id {
                m.internet_id = id;
            }
        }))
    }

    /// `POST /me/messages/{id}/createReply|createReplyAll|createForward`.
    pub(super) fn create_answer(&mut self, asked: &Asked, id: &str, action: &str) -> Result<Answer, Refusal> {
        let id = State::message_id(id)?;
        let look = State::look_of(asked, false)?;
        let body = if asked.body.trim().is_empty() { json!({}) } else { asked.json() };
        let map = body.as_object().ok_or_else(|| bad("The body of an action is a JSON object."))?;
        if map.keys().any(|key| !matches!(key.as_str(), "comment" | "message" | "toRecipients")) {
            return Err(refuse(400, "RequestBodyRead", "An answer is made with a comment or a message, and nothing else."));
        }
        let comment = map.get("comment").and_then(Value::as_str).unwrap_or_default().to_string();
        let message = map.get("message").cloned().unwrap_or(Value::Null);
        if !comment.is_empty() && message.get("body").is_some() {
            return Err(bad("Specify either a comment or a body of the message, not both."));
        }
        let source = self.mailbox.messages.get(&id).cloned().ok_or_else(not_found)?;
        self.fault(&id)?;
        // What the message parameter says, checked before anything is made.
        let mut written = Written::default();
        let mut extended = Vec::new();
        if let Some(map) = message.as_object() {
            for (name, value) in map {
                if written.take(name, value)? {
                    continue;
                }
                match name.as_str() {
                    "singleValueExtendedProperties" => extended = super::changes::extended_of(value)?,
                    other => return Err(refuse(400, "RequestBodyRead", format!("The property '{other}' is not one an answer is made with here."))),
                }
            }
        }
        let owner = self.owner.clone();
        let (prefix, forward) = match action {
            "createForward" => ("FW: ", true),
            _ => ("RE: ", false),
        };
        let quote = format!(
            "<html><body><p>{comment}</p><hr><p><b>From:</b> {}<br><b>Subject:</b> {}</p><div>{}</div></body></html>",
            source.from,
            source.subject,
            source.text()
        );
        let mut attachments = Vec::new();
        if forward {
            for a in &source.attachments {
                let (aid, _) = self.mailbox.new_id();
                attachments.push(Attached { id: format!("{aid}-att"), ..a.clone() });
            }
        }
        let made = self.new_draft(|m| {
            m.subject = format!("{prefix}{}", source.subject);
            m.conversation = source.conversation_id();
            if !forward {
                m.in_reply_to = source.internet_id.clone();
                m.to = vec![source.from.clone()];
                if action == "createReplyAll" {
                    m.to.extend(source.to.iter().filter(|a| !a.eq_ignore_ascii_case(&owner)).cloned());
                    m.cc = source.cc.iter().filter(|a| !a.eq_ignore_ascii_case(&owner)).cloned().collect();
                }
            }
            (m.html, m.body) = (true, quote);
            m.preview = preview_of(&m.body, true);
            m.attachments = attachments;
            written.apply(m);
            m.extended = extended;
        });
        Ok((201, Some(self.message_json(&made, &look))))
    }

    /// `/me/messages/{id}/attachments[/{attachment}]`.
    pub(super) fn attachments_at(&mut self, asked: &Asked, method: &str, id: &str, attachment: Option<&str>) -> Result<Answer, Refusal> {
        let id = State::message_id(id)?;
        if !self.mailbox.messages.contains_key(&id) {
            return Err(not_found());
        }
        let find = |m: &Message, aid: &str| m.attachments.iter().position(|a| a.id == aid);
        match (method, attachment) {
            ("GET", None) => {
                if let Some((other, _)) = asked.query.iter().find(|(name, _)| !matches!(name.as_str(), "$select" | "$top" | "$skip")) {
                    return Err(bad(format!("The query parameter '{other}' is not one this stand-in answers here.")));
                }
                let select: Option<Vec<String>> = asked.param("$select").map(|names| names.split(',').map(|n| n.trim().to_string()).collect());
                const NAMES: &[&str] = &["id", "name", "contentType", "size", "isInline", "contentId", "lastModifiedDateTime"];
                if let Some(unknown) = select.iter().flatten().find(|name| !NAMES.contains(&name.as_str())) {
                    return Err(refuse(
                        400,
                        "BadRequest",
                        format!("Could not find a property named '{unknown}' on type 'microsoft.graph.attachment'."),
                    ));
                }
                let list: Vec<Value> =
                    self.mailbox.messages.get(&id).map(|m| m.attachments.iter().map(|a| a.json(false)).collect()).unwrap_or_default();
                let list = list
                    .into_iter()
                    .map(|mut one| {
                        if let (Some(select), Some(map)) = (&select, one.as_object_mut()) {
                            map.retain(|name, _| name == "id" || name == "@odata.type" || select.iter().any(|s| s == name));
                        }
                        one
                    })
                    .collect();
                self.page(asked, list)
            }
            ("GET", Some(aid)) => {
                let m = self.mailbox.messages.get(&id).ok_or_else(not_found)?;
                let at = find(m, aid).ok_or_else(not_found)?;
                Ok((200, m.attachments.get(at).map(|a| a.json(true))))
            }
            ("POST", None) => {
                let attached = State::attachment_of(&asked.json())?;
                self.fault(&id)?;
                let (n, _) = self.mailbox.new_id();
                let attached = Attached { id: format!("{n}-att"), ..attached };
                let answer = attached.json(false);
                self.mailbox.update(&id, |m| m.attachments.push(attached));
                Ok((201, Some(answer)))
            }
            ("DELETE", Some(aid)) => {
                let at = self.mailbox.messages.get(&id).and_then(|m| find(m, aid)).ok_or_else(not_found)?;
                self.fault(&id)?;
                self.mailbox.update(&id, |m| {
                    m.attachments.remove(at);
                });
                Ok((204, None))
            }
            _ => Err(refuse(405, "MethodNotAllowed", "The method is not allowed on this resource.")),
        }
    }

    /// A `fileAttachment` as a request to add one writes it.
    fn attachment_of(body: &Value) -> Result<Attached, Refusal> {
        let map: &Map<String, Value> = body.as_object().ok_or_else(|| bad("An attachment is a JSON object."))?;
        if map.get("@odata.type").and_then(Value::as_str) != Some("#microsoft.graph.fileAttachment") {
            return Err(bad("An attachment says its @odata.type: #microsoft.graph.fileAttachment."));
        }
        let mut out = Attached::default();
        for (name, value) in map {
            match name.as_str() {
                "@odata.type" => {}
                "name" => out.name = value.as_str().filter(|n| !n.trim().is_empty()).ok_or_else(|| bad("An attachment has a name."))?.to_string(),
                "contentType" => out.content_type = value.as_str().unwrap_or_default().to_string(),
                "isInline" => out.inline = value.as_bool().ok_or_else(|| bad("'isInline' is true or false."))?,
                "contentId" => out.content_id = value.as_str().unwrap_or_default().to_string(),
                "contentBytes" => {
                    let text = value.as_str().ok_or_else(|| bad("'contentBytes' is base64."))?;
                    out.bytes = STANDARD.decode(text.as_bytes()).map_err(|_| bad("'contentBytes' is base64."))?;
                }
                other => {
                    return Err(refuse(
                        400,
                        "RequestBodyRead",
                        format!("The property '{other}' does not exist on type 'microsoft.graph.fileAttachment'."),
                    ));
                }
            }
        }
        if out.name.is_empty() || !map.contains_key("contentBytes") {
            return Err(bad("An attachment has a name and its contentBytes."));
        }
        if out.bytes.len() >= DIRECT_MOST {
            return Err(refuse(413, "ErrorRequestEntityTooLarge", "An attachment of 3 MB or more is added through an upload session."));
        }
        Ok(out)
    }

    /// `POST /me/messages/{id}/attachments/createUploadSession`.
    pub(super) fn upload_session(&mut self, asked: &Asked, id: &str) -> Result<Answer, Refusal> {
        let id = State::message_id(id)?;
        if !self.mailbox.messages.contains_key(&id) {
            return Err(not_found());
        }
        let body = asked.json();
        let item = body.get("AttachmentItem").and_then(Value::as_object).ok_or_else(|| bad("An upload session is made for an AttachmentItem."))?;
        if body.as_object().is_some_and(|map| map.len() != 1) {
            return Err(refuse(400, "RequestBodyRead", "An upload session is made for an AttachmentItem and nothing else."));
        }
        let mut attached = Attached::default();
        let mut size = None;
        for (name, value) in item {
            match name.as_str() {
                "attachmentType" if value.as_str() == Some("file") => {}
                "attachmentType" => return Err(bad("'attachmentType' is file.")),
                "name" => {
                    attached.name = value.as_str().filter(|n| !n.trim().is_empty()).ok_or_else(|| bad("An attachment has a name."))?.to_string()
                }
                "size" => size = value.as_u64(),
                "contentType" => attached.content_type = value.as_str().unwrap_or_default().to_string(),
                "isInline" => attached.inline = value.as_bool().ok_or_else(|| bad("'isInline' is true or false."))?,
                "contentId" => attached.content_id = value.as_str().unwrap_or_default().to_string(),
                other => {
                    return Err(refuse(
                        400,
                        "RequestBodyRead",
                        format!("The property '{other}' does not exist on type 'microsoft.graph.attachmentItem'."),
                    ));
                }
            }
        }
        let size = size.ok_or_else(|| bad("An AttachmentItem says its size."))?;
        if attached.name.is_empty() {
            return Err(bad("An attachment has a name."));
        }
        if size < SESSION_LEAST {
            return Err(refuse(400, "ErrorAttachmentSizeShouldNotBeLessThanMinimumSize", "Attachment size must be at least 3 MB."));
        }
        if size > SESSION_MOST {
            return Err(refuse(400, "ErrorAttachmentSizeShouldNotBeGreaterThanMaximumSize", "Attachment size must be at most 150 MB."));
        }
        self.fault(&id)?;
        let (_, n) = self.mailbox.new_id();
        let token = format!("us-{n:012x}-{:x}", self.clock);
        self.mailbox.uploads.insert(token.clone(), Upload { message: id, attached, size });
        let url = format!("{}/upload/{token}", self.base);
        Ok((201, Some(json!({"uploadUrl": url, "expirationDateTime": "2027-01-16T08:00:00Z", "nextExpectedRanges": ["0-"]}))))
    }

    /// `PUT {uploadUrl}`: a piece of a large attachment; the last makes it. `DELETE` gives up.
    pub(super) fn upload_piece(&mut self, asked: &Asked, token: &str) -> Result<Answer, Refusal> {
        if !asked.header("authorization").is_empty() {
            return Err(refuse(
                401,
                "InvalidAuthenticationToken",
                "An upload URL is its own permission: a request to it carries no Authorization header.",
            ));
        }
        if asked.method == "DELETE" {
            return match self.mailbox.uploads.remove(token) {
                Some(_) => Ok((204, None)),
                None => Err(not_found()),
            };
        }
        if asked.method != "PUT" {
            return Err(refuse(405, "MethodNotAllowed", "The pieces of an upload are PUT."));
        }
        let upload = self.mailbox.uploads.get(token).cloned().ok_or_else(not_found)?;
        let range = asked.header("content-range").to_string();
        let parsed = range.strip_prefix("bytes ").and_then(|r| {
            let (span, total) = r.split_once('/')?;
            let (from, to) = span.split_once('-')?;
            Some((from.trim().parse::<u64>().ok()?, to.trim().parse::<u64>().ok()?, total.trim().parse::<u64>().ok()?))
        });
        let (from, to, total) = parsed.ok_or_else(|| bad("A piece of an upload says its Content-Range: bytes from-to/total."))?;
        let had = upload.attached.bytes.len() as u64;
        if total != upload.size || to < from || to >= total {
            return Err(bad("The Content-Range doesn't fit the upload."));
        }
        if from != had {
            return Err(refuse(416, "InvalidRange", format!("The next piece expected starts at {had}.")));
        }
        let piece = &asked.bytes;
        if piece.len() as u64 != to - from + 1 {
            return Err(bad("The piece isn't as long as its Content-Range says."));
        }
        if piece.len() > PIECE_MOST {
            return Err(refuse(413, "ErrorRequestEntityTooLarge", "A piece of an upload is 4 MB at most."));
        }
        let mut upload = upload;
        upload.attached.bytes.extend_from_slice(piece);
        if (upload.attached.bytes.len() as u64) < upload.size {
            let next = upload.attached.bytes.len();
            self.mailbox.uploads.insert(token.to_string(), upload);
            return Ok((200, Some(json!({"expirationDateTime": "2027-01-16T08:00:00Z", "nextExpectedRanges": [format!("{next}-")]}))));
        }
        self.mailbox.uploads.remove(token);
        let message = upload.message.clone();
        if !self.mailbox.messages.contains_key(&message) {
            return Err(not_found());
        }
        let (n, _) = self.mailbox.new_id();
        let attached = Attached { id: format!("{n}-att"), ..upload.attached };
        self.location = Some(format!("{}/v1.0/me/messages('{message}')/attachments('{}')", self.base, attached.id));
        self.mailbox.update(&message, |m| m.attachments.push(attached));
        Ok((201, None))
    }
}

/// `internetMessageHeaders`: `[{"name", "value"}]`, every name beginning with `x-`.
fn headers_of(value: &Value) -> Result<Vec<(String, String)>, Refusal> {
    let list = value.as_array().ok_or_else(|| bad("'internetMessageHeaders' is a list of names and values."))?;
    list.iter()
        .map(|one| {
            let (Some(name), Some(value)) = (one.get("name").and_then(Value::as_str), one.get("value").and_then(Value::as_str)) else {
                return Err(bad("'internetMessageHeaders' is a list of names and values."));
            };
            if !name.to_ascii_lowercase().starts_with("x-") {
                return Err(refuse(
                    400,
                    "InvalidInternetMessageHeader",
                    format!("The internet message header name '{name}' should start with 'x-' or 'X-'."),
                ));
            }
            Ok((name.to_string(), value.to_string()))
        })
        .collect()
}

#[cfg(test)]
mod tests;
