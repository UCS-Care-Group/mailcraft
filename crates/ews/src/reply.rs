// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Judging an answer: the HTTP status first, then the SOAP envelope, a fault, and the response
//! messages with their classes and codes.
//!
//! The elements that matter are found by their namespace and local name, never by a prefix.
//! Everything else in an answer (an element this crate does not know, a header, a remark) is
//! skipped.

use std::fmt;

use crate::error::{DEFAULT_BACK_OFF_MS, Error, MAX_BACK_OFF_MS, MIN_BACK_OFF_MS, PolicyReason, ResponseClass, ResponseCode};
use crate::http::HttpResponse;
use crate::soap::{MESSAGES, SOAP};
use crate::xml::{self, Node};

/// The body of a SOAP answer that was not a fault.
pub struct Reply {
    body: Node,
}

impl fmt::Debug for Reply {
    /// Nothing of the answer: it may hold someone's mail.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Reply(..)")
    }
}

/// One response message of an operation's answer.
pub struct Message<'n> {
    class: ResponseClass,
    code: ResponseCode,
    /// The server says it is busy (`ErrorServerBusy`, or inside `ErrorInternalServerError`).
    busy: bool,
    back_off_ms: Option<u32>,
    node: &'n Node,
}

impl fmt::Debug for Message<'_> {
    /// The class and the code: the rest is the server's.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Message").field("class", &self.class).field("code", &self.code).finish()
    }
}

impl Reply {
    /// Judges `response`: the status, then the document. `limit` is the most body there may be.
    pub fn judge(response: &HttpResponse, limit: usize) -> Result<Self, Error> {
        match response.status() {
            401 => return Err(Error::Unauthorized),
            403 => {
                return Err(match response.policy_reason() {
                    Some(reason) => Error::Blocked(PolicyReason::from_header(reason)),
                    None => Error::Forbidden,
                });
            }
            429 | 503 => return Err(Error::Busy { back_off_ms: retry_after_ms(response.retry_after()) }),
            200 | 500 => {}
            other => return Err(Error::Status(other)),
        }
        if response.body().len() > limit {
            return Err(Error::TooLarge);
        }
        match Self::parse(response.body()) {
            // (A 500 that is no fault is no answer of the service.)
            Ok(_) if response.status() == 500 => Err(Error::Status(500)),
            Err(Error::Malformed | Error::Unexpected) if response.status() == 500 => Err(Error::Status(500)),
            other => other,
        }
    }

    /// Reads the document `body`: a SOAP envelope whose body is not a fault. A fault is the
    /// error it names.
    pub fn parse(body: &[u8]) -> Result<Self, Error> {
        let root = xml::parse(body)?;
        if !root.is(SOAP, "Envelope") {
            return Err(Error::Unexpected);
        }
        let body = root.children.into_iter().find(|child| child.is(SOAP, "Body")).ok_or(Error::Unexpected)?;
        if let Some(fault) = body.child_in(SOAP, "Fault") {
            return Err(read_fault(fault));
        }
        Ok(Self { body })
    }

    /// The busy error, when any response message of the answer says the server is busy: it is
    /// the whole request that is to be made again, whatever the operation, and a busy server
    /// answers one message however many questions there were.
    pub fn busy(&self) -> Option<Error> {
        let response = self.body.children.first()?;
        let all = response.child_in(MESSAGES, "ResponseMessages")?;
        all.children.iter().find_map(|child| Message::read(child).ok()?.ok().err().filter(|error| matches!(error, Error::Busy { .. })))
    }

    /// The response messages of the answer to `operation` (`GetFolder`: the
    /// `GetFolderResponseMessage`s inside `GetFolderResponse`), in order.
    pub fn messages(&self, operation: &str) -> Result<Vec<Message<'_>>, Error> {
        let response = self.body.child_in(MESSAGES, &format!("{operation}Response")).ok_or(Error::Unexpected)?;
        let all = response.child_in(MESSAGES, "ResponseMessages").ok_or(Error::Unexpected)?;
        let name = format!("{operation}ResponseMessage");
        all.children.iter().filter(|child| child.is(MESSAGES, &name)).map(Message::read).collect()
    }
}

impl<'n> Message<'n> {
    fn read(node: &'n Node) -> Result<Self, Error> {
        let class = node.attribute("ResponseClass").and_then(ResponseClass::from_name).ok_or(Error::Unexpected)?;
        let code = node.child_in(MESSAGES, "ResponseCode").map(|code| ResponseCode::from_name(&code.text)).ok_or(Error::Unexpected)?;
        // (`MessageXml` and its values are told by their local names: a server puts them in the
        // messages' namespace or the types'.)
        let (busy, back_off_ms) = busy_of(&code, node.child("MessageXml"), node.child("BackOffMilliseconds").map(|wait| wait.text.as_str()));
        Ok(Self { class, code, busy, back_off_ms, node })
    }

    /// How the server says it went.
    pub fn class(&self) -> ResponseClass {
        self.class
    }

    /// The code the server gave.
    pub fn code(&self) -> &ResponseCode {
        &self.code
    }

    /// The element, for whoever reads what it holds.
    pub fn node(&self) -> &'n Node {
        self.node
    }

    /// The element when the message says it was done; the error it names when it was not.
    pub fn ok(&self) -> Result<&'n Node, Error> {
        if self.class != ResponseClass::Error {
            return Ok(self.node);
        }
        Err(if self.busy { Error::Busy { back_off_ms: self.back_off_ms.unwrap_or(DEFAULT_BACK_OFF_MS) } } else { Error::Server(self.code.clone()) })
    }
}

/// The text of the `Value` called `name` inside a `MessageXml`.
fn value_of<'a>(message_xml: &'a Node, name: &str) -> Option<&'a str> {
    message_xml.children.iter().find(|value| value.name == "Value" && value.attribute("Name") == Some(name)).map(|value| value.text.trim())
}

/// Whether a message with this `code` and `MessageXml` says the server is busy, and the wait
/// it asks for. Busy is `ErrorServerBusy`, and also `ErrorInternalServerError` whose inner
/// error is `ErrorServerBusy` (Exchange's throttling says so in a 200 answer). The wait is a
/// `BackOffMilliseconds` of its own or a `Value` of that name.
fn busy_of(code: &ResponseCode, message_xml: Option<&Node>, direct: Option<&str>) -> (bool, Option<u32>) {
    let inner = message_xml.and_then(|xml| value_of(xml, "InnerErrorResponseCode")).map(ResponseCode::from_name);
    let busy = *code == ResponseCode::ServerBusy || (*code == ResponseCode::InternalServerError && inner == Some(ResponseCode::ServerBusy));
    let wait = direct.or_else(|| message_xml.and_then(|xml| value_of(xml, "BackOffMilliseconds"))).map(back_off_ms);
    (busy, wait)
}

/// A wait in milliseconds, as a server writes it: digits, at least [`MIN_BACK_OFF_MS`] and at
/// most [`MAX_BACK_OFF_MS`]; anything else is the default.
fn back_off_ms(text: &str) -> u32 {
    let text = text.trim();
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return DEFAULT_BACK_OFF_MS;
    }
    // (A number of any length: too many digits is simply the most.)
    let ms = text.parse::<u64>().unwrap_or(u64::MAX);
    u32::try_from(ms.clamp(u64::from(MIN_BACK_OFF_MS), u64::from(MAX_BACK_OFF_MS))).unwrap_or(MAX_BACK_OFF_MS)
}

/// A `Retry-After` in seconds (a date is not read: the default), as milliseconds, within the
/// same bounds.
fn retry_after_ms(value: Option<&str>) -> u32 {
    let text = value.unwrap_or("").trim();
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return DEFAULT_BACK_OFF_MS;
    }
    let ms = text.parse::<u64>().unwrap_or(u64::MAX).saturating_mul(1000);
    u32::try_from(ms.clamp(u64::from(MIN_BACK_OFF_MS), u64::from(MAX_BACK_OFF_MS))).unwrap_or(MAX_BACK_OFF_MS)
}

/// The error a `Fault` names: the response code of its `detail`, and for a busy server the wait.
/// (`ResponseCode` and `MessageXml` are told by their local names, and `Value` too: the service
/// writes `MessageXml` in the types' namespace as often as the errors'.)
fn read_fault(fault: &Node) -> Error {
    let detail = fault.child("detail");
    let code = detail
        .and_then(|detail| detail.child("ResponseCode"))
        .map_or_else(|| ResponseCode::Other(String::new()), |code| ResponseCode::from_name(&code.text));
    let (busy, wait) = busy_of(&code, detail.and_then(|detail| detail.child("MessageXml")), None);
    if busy { Error::Busy { back_off_ms: wait.unwrap_or(DEFAULT_BACK_OFF_MS) } } else { Error::Server(code) }
}
