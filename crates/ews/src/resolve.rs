// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Looking a name up in the organisation's address list (#1262): the operation `ResolveNames`
//! of [MS-OXWSRSLNM], with `SearchScope="ActiveDirectory"`, so that only the directory is searched
//! and never the mailbox's own contacts.
//!
//! What is sent is the text the owner typed and nothing else. What comes back is read as hostile
//! as everything else a server says: at most [`MAX_CANDIDATES`] people, each a name, an SMTP
//! address, what kind of recipient it is, three short lines that tell two people of one name
//! apart (company, department, job title), and for the Address Book in People how they are
//! reached (the business and mobile phone numbers, the office). Nothing else the server wrote is
//! kept: no postal addresses, notes or other numbers, though the answer may hold them; and what is
//! read lives in the window, never in the mailbox.

use crate::error::{Error, ResponseClass, ResponseCode};
use crate::reply::Reply;
use crate::soap::{Element, MESSAGES, TYPES};
use crate::xml::Node;

/// Most people one answer is read for; a server sends at most 100 (the operation's own limit).
pub const MAX_CANDIDATES: usize = 100;
/// The longest text looked up, in characters.
pub const MAX_LOOKUP: usize = 256;
/// The longest name kept.
const MAX_NAME: usize = 256;
/// The longest address kept (RFC 5321's path, less the brackets).
const MAX_ADDRESS: usize = 254;
/// The longest company, department, job title or office kept.
const MAX_DETAIL: usize = 128;
/// The longest phone number kept, in characters.
const MAX_PHONE: usize = 40;

/// What kind of recipient a candidate is ([MS-OXWSCDATA] `MailboxTypeType`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum RecipientKind {
    /// A person's mailbox (`Mailbox`, `User`).
    #[default]
    Mailbox,
    /// A distribution list (`PublicDL`, `PrivateDL`); the server expands it when mail is sent.
    List,
    /// A mail contact: somebody outside the organisation in its address list (`Contact`).
    Contact,
    /// A Microsoft 365 group (`GroupMailbox`).
    Group,
    /// A public folder that takes mail (`PublicFolder`).
    PublicFolder,
    /// Any other kind (`OneOff`, `Unknown`, a name not in the schema).
    Other,
}

impl RecipientKind {
    fn from_name(name: &str) -> Self {
        match name.trim() {
            "Mailbox" | "User" => Self::Mailbox,
            "PublicDL" | "PrivateDL" => Self::List,
            "Contact" | "ImplicitContact" => Self::Contact,
            "GroupMailbox" => Self::Group,
            "PublicFolder" => Self::PublicFolder,
            _ => Self::Other,
        }
    }
}

/// One person (or list) the address list has for the text looked up.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Candidate {
    /// The name the directory shows.
    pub name: String,
    /// The SMTP address.
    pub email: String,
    /// What kind of recipient it is.
    pub kind: RecipientKind,
    /// The company, as far as the directory says it.
    pub company: String,
    /// The department.
    pub department: String,
    /// The job title.
    pub title: String,
    /// The business phone number, as the directory writes it.
    pub phone: String,
    /// The mobile phone number.
    pub mobile: String,
    /// Where their office is.
    pub office: String,
}

impl std::fmt::Debug for Candidate {
    // (Who was found is the owner's business: a log line says what kind, never who.)
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Candidate").field("kind", &self.kind).finish_non_exhaustive()
    }
}

/// What the address list answered: the people it has for the text, and whether it has more
/// than it listed (the text is to be made longer then).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Resolved {
    /// The people, each address once, in the server's order.
    pub candidates: Vec<Candidate>,
    /// The server has more than it listed, or than were read.
    pub more: bool,
}

/// The request `ResolveNames` for one text, looked up in the organisation's address list only.
#[derive(Clone, PartialEq, Eq)]
pub struct ResolveNames {
    text: String,
}

impl std::fmt::Debug for ResolveNames {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolveNames").field("text", &self.text.chars().count()).finish()
    }
}

impl ResolveNames {
    /// Looks `text` up: one line of it, at most [`MAX_LOOKUP`] characters. None when nothing is
    /// left to look up.
    pub fn new(text: &str) -> Option<Self> {
        let text = one_line(text, MAX_LOOKUP);
        (!text.is_empty()).then_some(Self { text })
    }

    /// The text that is sent.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The operation as the schema has it: `ReturnFullContactData` (for the company, department
    /// and job title), `SearchScope` `ActiveDirectory`, and the `UnresolvedEntry`.
    pub fn element(&self) -> Element {
        Element::new("m:ResolveNames")
            .attr("ReturnFullContactData", "true")
            .attr("SearchScope", "ActiveDirectory")
            .child(Element::new("m:UnresolvedEntry").text(&self.text))
    }

    /// The people of the answer. Nobody found is an empty answer, not an error; several found is
    /// the warning `ErrorNameResolutionMultipleResults`, and is read as one. A busy server is
    /// [`Error::Busy`], any other refusal [`Error::Server`].
    pub fn answer(&self, reply: &Reply) -> Result<Resolved, Error> {
        let messages = reply.messages("ResolveNames")?;
        let message = messages.first().ok_or(Error::Unexpected)?;
        if message.class() == ResponseClass::Error && is_code(message.code(), "ErrorNameResolutionNoResults") {
            return Ok(Resolved::default());
        }
        let node = message.ok()?;
        let Some(set) = node.child_in(MESSAGES, "ResolutionSet") else { return Ok(Resolved::default()) };
        let mut out = Resolved::default();
        let mut seen: Vec<String> = Vec::new();
        let mut read = 0usize;
        for resolution in set.children.iter().filter(|child| child.is(TYPES, "Resolution")) {
            if read >= MAX_CANDIDATES {
                out.more = true;
                break;
            }
            read = read.saturating_add(1);
            let Some(candidate) = candidate(resolution) else { continue };
            let key = candidate.email.to_lowercase();
            if seen.contains(&key) {
                continue;
            }
            seen.push(key);
            out.candidates.push(candidate);
        }
        let last = set.attribute("IncludesLastItemInRange").map(str::trim);
        let total = set.attribute("TotalItemsInView").and_then(|n| n.trim().parse::<usize>().ok());
        out.more |= last == Some("false") || total.is_some_and(|total| total > read);
        Ok(out)
    }
}

fn is_code(code: &ResponseCode, name: &str) -> bool {
    matches!(code, ResponseCode::Other(other) if other == name)
}

/// The person of a `Resolution`: its `Mailbox`, with what its `Contact` says of them. One with
/// no SMTP address that is an address is passed over: there is nothing to send to.
fn candidate(resolution: &Node) -> Option<Candidate> {
    let mailbox = resolution.child_in(TYPES, "Mailbox")?;
    let routing = mailbox.child_in(TYPES, "RoutingType").map_or("SMTP", |n| n.text.trim());
    if !routing.eq_ignore_ascii_case("SMTP") {
        return None;
    }
    let email = mailbox.child_in(TYPES, "EmailAddress").map(|n| n.text.trim()).filter(|a| is_address(a))?.to_owned();
    let contact = resolution.child_in(TYPES, "Contact");
    let of_contact = |name: &str, most: usize| one_line(contact.and_then(|c| c.child_in(TYPES, name)).map_or("", |n| n.text.as_str()), most);
    let mut name = of_contact("DisplayName", MAX_NAME);
    if name.is_empty() {
        name = one_line(mailbox.child_in(TYPES, "Name").map_or("", |n| n.text.as_str()), MAX_NAME);
    }
    Some(Candidate {
        name,
        email,
        kind: RecipientKind::from_name(mailbox.child_in(TYPES, "MailboxType").map_or("", |n| n.text.as_str())),
        company: of_contact("CompanyName", MAX_DETAIL),
        department: of_contact("Department", MAX_DETAIL),
        title: of_contact("JobTitle", MAX_DETAIL),
        phone: phone_of(contact, "BusinessPhone"),
        mobile: phone_of(contact, "MobilePhone"),
        office: of_contact("OfficeLocation", MAX_DETAIL),
    })
}

/// The contact's phone number of `key` (`BusinessPhone`, `MobilePhone`): kept only when it is
/// written as a phone number is (digits, at least three, with spaces, `+`, `-`, `.`, `/`,
/// brackets, and an extension's `x`), so that what is shown and dialled is a number and nothing
/// else.
fn phone_of(contact: Option<&Node>, key: &str) -> String {
    let Some(numbers) = contact.and_then(|c| c.child_in(TYPES, "PhoneNumbers")) else { return String::new() };
    let Some(entry) = numbers.children.iter().find(|e| e.is(TYPES, "Entry") && e.attribute("Key").map(str::trim) == Some(key)) else {
        return String::new();
    };
    let number = one_line(&entry.text, MAX_PHONE.saturating_add(1));
    let digits = number.chars().filter(char::is_ascii_digit).count();
    let shaped = number.chars().all(|c| c.is_ascii_digit() || matches!(c, ' ' | '+' | '-' | '.' | '/' | '(' | ')' | 'x' | 'X'));
    if shaped && digits >= 3 && number.chars().count() <= MAX_PHONE { number } else { String::new() }
}

/// Whether `text` is an address mail can go to: one `@` with something on both sides, no
/// white space or control characters, no brackets or quotes, and not too long.
fn is_address(text: &str) -> bool {
    let Some((local, domain)) = text.split_once('@') else { return false };
    !local.is_empty()
        && !domain.is_empty()
        && !domain.contains('@')
        && text.chars().count() <= MAX_ADDRESS
        && !text.chars().any(|c| c.is_whitespace() || c.is_control() || matches!(c, '<' | '>' | '"' | ',' | ';' | '(' | ')'))
}

/// `text` as one line: white space and control characters runs become one space, the ends are
/// trimmed, and at most `most` characters are kept.
fn one_line(text: &str, most: usize) -> String {
    let mut out = String::new();
    let mut count = 0usize;
    for word in text.split(|c: char| c.is_whitespace() || c.is_control()).filter(|w| !w.is_empty()) {
        if count >= most {
            break;
        }
        if !out.is_empty() {
            out.push(' ');
            count = count.saturating_add(1);
        }
        for c in word.chars() {
            if count >= most {
                break;
            }
            out.push(c);
            count = count.saturating_add(1);
        }
    }
    out.trim_end().to_owned()
}
