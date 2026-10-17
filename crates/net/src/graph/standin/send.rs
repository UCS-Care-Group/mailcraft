//! The stand-in's sending, as Graph v1.0 documents it ("user: sendMail", "message: send"):
//!
//! - `POST /v1.0/me/sendMail` sends a message never saved as a draft: from JSON (`{"message":
//!   {…}, "saveToSentItems"}`, the message as a new one is written) or from MIME
//!   (`Content-Type: text/plain`, the message in base64). Answered 202 with no body.
//! - `POST /v1.0/me/messages/{id}/send` sends a draft (400 for a message that isn't one), with
//!   no body. Answered 202. The draft leaves Drafts.
//!
//! Another mailbox (#70) sends the same way at `/v1.0/users/{address}/…`, its copy in its own
//! Sent Items; one whose owner of the token may not send as it nor on its behalf answers 403
//! `ErrorSendAsDenied` ([`super::OtherMailbox::send_as_denied`]) and sends nothing.
//!
//! Either way the server files its own copy in Sent Items (unless `saveToSentItems` is false),
//! under an id of its own, which a client finds by the delta of Sent Items; and it keeps what it
//! sent ([`State::sent`](super::State::sent)), for a test to count. Faults: a message's own
//! ([`super::Fault`]), [`State::fail_next`](super::State::fail_next) and
//! [`State::lose_next`](super::State::lose_next) (sent, and the answer lost).

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde_json::Value;

use super::messages::Message;
use super::{Answer, Asked, Refusal, State, bad, not_found, refuse};

/// A message the stand-in sent: its subject, its recipients, and how it was asked to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Sent {
    pub subject: String,
    pub to: Vec<String>,
    /// `sendMail` or `send` (a draft).
    pub how: String,
    /// It said it answers this Message-ID (`In-Reply-To`).
    pub in_reply_to: String,
    /// Whose mailbox sent it: `me`, or `users/{address}` for another (#70), whose Sent Items
    /// has the copy.
    pub mailbox: String,
}

impl State {
    /// File `m` in Sent Items as sent, under an id of its own, and keep a word of it.
    fn file_sent(&mut self, mut m: Message, how: &str, save: bool) {
        let mailbox = self.whose().to_string();
        self.sent.push(Sent { subject: m.subject.clone(), to: m.to.clone(), how: how.into(), in_reply_to: m.in_reply_to.clone(), mailbox });
        if m.to.is_empty() && m.cc.is_empty() && m.bcc.is_empty() {
            return;
        }
        if save {
            m.folder = self.well_known("sentitems");
            (m.draft, m.read, m.extended) = (false, true, Vec::new());
            m.received = self.clock;
            self.mailbox.put(m);
        }
    }

    /// `POST /me/sendMail`.
    pub(super) fn send_mail(&mut self, asked: &Asked) -> Result<Answer, Refusal> {
        let mime = asked.header("content-type").to_ascii_lowercase().starts_with("text/plain");
        let (m, save) = if mime {
            let raw = STANDARD
                .decode(asked.body.trim().as_bytes())
                .map_err(|_| refuse(400, "ErrorMimeContentInvalidBase64String", "The MIME content is not valid base64."))?;
            (self.message_of_mime(&String::from_utf8_lossy(&raw))?, true)
        } else {
            let body = asked.json();
            let map = body.as_object().ok_or_else(|| bad("sendMail takes a message."))?;
            if map.keys().any(|key| !matches!(key.as_str(), "message" | "saveToSentItems")) {
                return Err(refuse(400, "RequestBodyRead", "sendMail takes a message and saveToSentItems, and nothing else."));
            }
            let save = map.get("saveToSentItems").map_or(Ok(true), |v| v.as_bool().ok_or_else(|| bad("'saveToSentItems' is true or false.")))?;
            let message = map.get("message").ok_or_else(|| bad("sendMail takes a message."))?;
            (self.message_of_json(message)?, save)
        };
        if m.to.is_empty() && m.cc.is_empty() && m.bcc.is_empty() {
            return Err(refuse(400, "ErrorInvalidRecipients", "At least one recipient is required."));
        }
        self.file_sent(m, "sendMail", save);
        Ok((202, None))
    }

    /// `POST /me/messages/{id}/send`.
    pub(super) fn send_draft(&mut self, asked: &Asked, id: &str) -> Result<Answer, Refusal> {
        let id = State::message_id(id)?;
        if !asked.body.trim().is_empty() && asked.json() != Value::Object(Default::default()) {
            return Err(bad("A draft is sent with no body."));
        }
        let m = self.mailbox.messages.get(&id).cloned().ok_or_else(not_found)?;
        if !m.draft {
            return Err(refuse(400, "ErrorInvalidRequest", "Only a draft can be sent."));
        }
        if m.to.is_empty() && m.cc.is_empty() && m.bcc.is_empty() {
            return Err(refuse(400, "ErrorInvalidRecipients", "At least one recipient is required."));
        }
        self.fault(&id)?;
        self.mailbox.delete(&id);
        self.file_sent(m, "send", true);
        Ok((202, None))
    }
}

#[cfg(test)]
mod tests;
