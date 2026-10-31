// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A stand-in for Exchange Web Services that behaves like the documented service, for the tests.
//! It runs inside the test (an [`HttpExchange`] that answers at once): nothing here leaves the
//! machine, and nothing here is in a build that isn't a test's.
//!
//! It holds a client to what the public EWS reference and the schemas of [MS-OXWSCDATA] and
//! [MS-OXWSFOLD] document:
//!
//! - **The request.** `POST` with the `Authorization` it was set up with (401 otherwise): a SOAP
//!   1.1 envelope whose header names a version the schema has (`RequestServerVersion`) and whose
//!   body is one operation. Every element has to be in the namespace it belongs to (the
//!   messages' or the types'), with the attributes it has and no others, its children in the
//!   order of the schema, and every required element and attribute present; what isn't so is a
//!   SOAP fault (`ErrorSchemaValidation`, HTTP 500), as the service answers a request that does
//!   not fit its schema.
//! - **The operations.** `GetFolder` (a `FolderShape` of a `BaseShape`, and `FolderIds` of
//!   `DistinguishedFolderId` and `FolderId`). Any operation that would change something is
//!   noted ([`State::writes`]) and refused: nothing a client of this crate does before it has
//!   the owner's say may ever be one.
//! - **Refusals.** An organisation that doesn't allow the service answers 403 with an
//!   `X-EWS-Policy-Reason` ([`State::blocked`]); a busy one answers `ErrorServerBusy` with its
//!   `BackOffMilliseconds` ([`State::busy`]).
//!
//! And it can misbehave on purpose ([`Hostile`]), for a client that has to take anything.

use std::future::{Future, ready};
use std::sync::{Mutex, MutexGuard};

use crate::http::{HttpExchange, HttpRequest, HttpResponse, TransportError};
use crate::soap::{ERRORS, MESSAGES, SOAP, TYPES, Version};
use crate::xml::{self, Node, escape};

/// The `Authorization` a stand-in takes unless it is told another.
pub const AUTHORIZATION: &str = "Bearer standin.ews-acc_ess.tok-9876543210";
/// The two reasons Microsoft documents for the `X-EWS-Policy-Reason` header.
pub const DISABLED: &str = "EWS is disabled for this user or tenant";
/// See [`DISABLED`].
pub const BLOCKED_BY_POLICY: &str = "EWS is blocked by policy for this user or tenant";

/// The names a `DistinguishedFolderId` may have (the reference's list).
const DISTINGUISHED: &[&str] = &[
    "calendar",
    "contacts",
    "deleteditems",
    "drafts",
    "inbox",
    "journal",
    "notes",
    "outbox",
    "sentitems",
    "tasks",
    "msgfolderroot",
    "publicfoldersroot",
    "root",
    "junkemail",
    "searchfolders",
    "voicemail",
    "recoverableitemsroot",
    "archiveroot",
    "archivemsgfolderroot",
    "archiveinbox",
    "syncissues",
    "conflicts",
    "localfailures",
    "serverfailures",
];
/// Operations of the service that change a mailbox.
const WRITES: &[&str] = &[
    "CreateItem",
    "UpdateItem",
    "DeleteItem",
    "MoveItem",
    "CopyItem",
    "CreateFolder",
    "UpdateFolder",
    "DeleteFolder",
    "MoveFolder",
    "CopyFolder",
    "EmptyFolder",
    "CreateAttachment",
    "DeleteAttachment",
    "MarkAllItemsAsRead",
];

/// A way of answering that no service should have, for a client that has to take anything.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hostile {
    /// A document that brings a DTD and entities of its own.
    Dtd,
    /// An answer that stops in the middle.
    CutShort,
    /// Something that isn't XML at all, with a status that says all is well.
    NotXml,
    /// Elements nested without end.
    Deep,
    /// More than any client reads (17 MB).
    Huge,
    /// A SOAP fault that names no reason.
    Fault,
    /// Fewer answers than there were questions.
    TooFew,
    /// A folder id that is not an id.
    BadId,
    /// Items whose ids are not ids, with subjects of any length.
    BadItems,
}

/// A folder of the mailbox.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Folder {
    /// The distinguished name it has (`inbox`); empty for none.
    pub distinguished: String,
    /// Its id.
    pub id: String,
    /// Its name.
    pub name: String,
    /// How many items it holds.
    pub total: u32,
    /// How many are unread.
    pub unread: u32,
    /// The id of the folder it lies in; empty for the top.
    pub parent: String,
    /// Its `FolderClass` (`IPF.Note` for a folder of mail).
    pub class: String,
}

/// A request that came in, and what it was answered with.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Asked {
    /// The operation (`GetFolder`); empty when the request named none.
    pub operation: String,
    /// The status of the answer.
    pub status: u16,
    /// The response code of the first answer in it (`NoError`), or of the fault.
    pub code: String,
    /// The whole body, as it came.
    pub body: String,
}

/// What a stand-in keeps and how it behaves.
#[derive(Debug)]
pub struct State {
    /// The `Authorization` it takes.
    pub authorization: String,
    /// The folders of the mailbox.
    pub folders: Vec<Folder>,
    /// The organisation doesn't allow the service: every request is answered 403, with this as
    /// the `X-EWS-Policy-Reason` ([`DISABLED`], [`BLOCKED_BY_POLICY`]).
    pub blocked: Option<String>,
    /// The next so many requests are told that the server is busy, asking for this many
    /// milliseconds (`BackOffMilliseconds`).
    pub busy: (u32, u64),
    /// The next request is answered with a fault of `ErrorServerBusy` (its wait is `busy.1`)
    /// instead.
    pub busy_fault: bool,
    /// The next busy answer (`busy`) says it as Exchange's throttling does: a 200 answer whose
    /// message is `ErrorInternalServerError` with an inner `ErrorServerBusy`.
    pub busy_inner: bool,
    /// Every answer from now on is this instead of what it should be.
    pub hostile: Option<Hostile>,
    /// Every request so far, in order.
    pub requests: Vec<Asked>,
    /// The operations that would have changed something, by name.
    pub writes: Vec<String>,
    /// The items of the mailbox, in the order they came.
    pub items: Vec<crate::standin_mail::Item>,
    /// The revision of the mailbox: every change to a folder or an item is one more.
    pub rev: u64,
    /// What changed in the folders, and in the items, by revision.
    pub folder_log: Vec<crate::standin_mail::FolderLog>,
    pub item_log: Vec<crate::standin_mail::ItemLog>,
    /// Every sync state carries this: raising it makes the server not know any it gave before
    /// (`ErrorInvalidSyncStateData`).
    pub epoch: u32,
    /// The server doesn't know the properties `item:Preview` and `item:Flag`
    /// (`ErrorInvalidPropertyRequest`).
    pub lacks_preview: bool,
    /// A server that ignores `MaxChangesReturned` and gives every change it has in one answer.
    pub ignores_max: bool,
    /// With [`Hostile::BadItems`]: only the items of this folder (its id) are bad; empty for all.
    pub bad_items_in: String,
    /// How many ids have been given out.
    pub issued: u32,
    /// The server has no follow-up flag: setting `item:Flag` is `ErrorInvalidPropertyForOperation`.
    pub lacks_flag: bool,
    /// The messages sent, as the MIME that went out (a draft sent as itself is written as MIME).
    pub outgoing: Vec<Vec<u8>>,
    /// The `DeleteType` of every item or folder delete, in order.
    pub deletes: Vec<String>,
    /// The listing of folders gives a folder's `FolderClass` only when the request asks for it
    /// (`folder:FolderClass` among the additional properties), as Grommunio does: its Default
    /// shape leaves it out (E8).
    pub class_only_when_asked: bool,
}

impl Default for State {
    fn default() -> Self {
        let names = [
            ("root", "Top of Information Store"),
            ("msgfolderroot", "Top of mail"),
            ("inbox", "Inbox"),
            ("drafts", "Drafts"),
            ("sentitems", "Sent Items"),
            ("deleteditems", "Deleted Items"),
            ("junkemail", "Junk Email"),
            ("outbox", "Outbox"),
        ];
        let id_of = |n: usize| format!("AAMkAGV4YW1wbGUtb3JnL2V3cy1zdGFuZC1pbgAuAAAAAACq{n:06}+Tk/Zm9sZGVyLWlkAAA=");
        let folders = names
            .iter()
            .enumerate()
            .map(|(n, (distinguished, name))| Folder {
                distinguished: (*distinguished).into(),
                id: id_of(n),
                name: (*name).into(),
                total: if *distinguished == "inbox" { 12 } else { 0 },
                unread: if *distinguished == "inbox" { 3 } else { 0 },
                // (The root has nothing above it, the top of the mail lies in the root, and the rest in the top of the mail.)
                parent: match n {
                    0 => String::new(),
                    1 => id_of(0),
                    _ => id_of(1),
                },
                class: if *distinguished == "root" || *distinguished == "msgfolderroot" { String::new() } else { "IPF.Note".into() },
            })
            .chain([Folder {
                distinguished: "calendar".into(),
                id: id_of(100),
                name: "Calendar".into(),
                total: 0,
                unread: 0,
                parent: id_of(1),
                class: "IPF.Appointment".into(),
            }])
            .collect();
        Self {
            authorization: AUTHORIZATION.into(),
            folders,
            blocked: None,
            busy: (0, 0),
            busy_fault: false,
            busy_inner: false,
            hostile: None,
            requests: Vec::new(),
            writes: Vec::new(),
            items: Vec::new(),
            rev: 0,
            folder_log: Vec::new(),
            item_log: Vec::new(),
            epoch: 0,
            lacks_preview: false,
            ignores_max: false,
            bad_items_in: String::new(),
            issued: 0,
            lacks_flag: false,
            outgoing: Vec::new(),
            deletes: Vec::new(),
            class_only_when_asked: false,
        }
    }
}

/// The stand-in: an exchange that answers like the service. Its state is open to the test.
#[derive(Debug, Default)]
pub struct StandIn {
    state: Mutex<State>,
}

impl StandIn {
    /// A stand-in with the default mailbox.
    pub fn new() -> Self {
        Self::default()
    }

    /// Its state, to read or to change.
    pub fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl HttpExchange for StandIn {
    fn exchange(&self, request: &HttpRequest) -> impl Future<Output = Result<HttpResponse, TransportError>> {
        ready(Ok(self.state().respond(request)))
    }
}

/// A refusal: the response code, and why.
pub(crate) struct Refusal {
    pub(crate) code: &'static str,
    pub(crate) why: String,
}

pub(crate) fn invalid(why: impl Into<String>) -> Refusal {
    Refusal { code: "ErrorSchemaValidation", why: why.into() }
}

// ---- holding a request to the schema ----

/// Checks of elements against the schema.
pub(crate) struct Check;

impl Check {
    /// `node` is the element `name` of the namespace `space`, with no attributes but `allowed`.
    pub(crate) fn is(node: &Node, space: &str, name: &str, allowed: &[&str]) -> Result<(), Refusal> {
        if !node.is(space, name) {
            return Err(invalid(format!("the element {name} of {space} was expected, not {}", node.name)));
        }
        match node.attributes.iter().find(|(have, _)| !allowed.contains(&have.as_str())) {
            Some((odd, _)) => Err(invalid(format!("{name} has no attribute {odd}"))),
            None => Ok(()),
        }
    }

    /// The children of `node`, which are all of the namespace `space` and come in the order of
    /// `order` (each at most once): the child of each name, or none.
    pub(crate) fn children<'c>(node: &'c Node, space: &str, order: &[&str]) -> Result<Vec<Option<&'c Node>>, Refusal> {
        Self::no_text(node)?;
        let mut found: Vec<Option<&'c Node>> = order.iter().map(|_| None).collect();
        let mut at = 0usize;
        for child in &node.children {
            match order.iter().position(|name| *name == child.name) {
                Some(place) if place >= at && found.get(place).is_some_and(Option::is_none) && *child.namespace == *space => {
                    if let Some(slot) = found.get_mut(place) {
                        *slot = Some(child);
                    }
                    at = place;
                }
                _ => return Err(invalid(format!("{} has no place for {} there", node.name, child.name))),
            }
        }
        Ok(found)
    }

    /// Element-only content: nothing but white space between the elements.
    pub(crate) fn no_text(node: &Node) -> Result<(), Refusal> {
        if node.text.trim().is_empty() { Ok(()) } else { Err(invalid(format!("{} holds elements, not text", node.name))) }
    }

    /// An element with text at most.
    pub(crate) fn no_children(node: &Node) -> Result<(), Refusal> {
        if node.children.is_empty() { Ok(()) } else { Err(invalid(format!("{} holds text, not elements", node.name))) }
    }

    /// An element with nothing inside it.
    pub(crate) fn empty(node: &Node) -> Result<(), Refusal> {
        if node.children.is_empty() && node.text.trim().is_empty() { Ok(()) } else { Err(invalid(format!("{} holds nothing", node.name))) }
    }

    pub(crate) fn required<'c>(node: &Node, child: Option<&'c Node>, name: &str) -> Result<&'c Node, Refusal> {
        child.ok_or_else(|| invalid(format!("{} needs a {name}", node.name)))
    }

    pub(crate) fn attribute<'c>(node: &'c Node, name: &str) -> Result<&'c str, Refusal> {
        node.attribute(name).ok_or_else(|| invalid(format!("{} needs the attribute {name}", node.name)))
    }
}

impl State {
    pub(crate) fn folder(&self, key: &str, by_name: bool) -> Option<&Folder> {
        self.folders.iter().find(|folder| if by_name { folder.distinguished == key } else { folder.id == key })
    }

    /// The `FolderIds` of a request: the folder asked for, or none the mailbox has.
    fn folder_ids(&self, node: &Node) -> Result<Vec<Option<Folder>>, Refusal> {
        Check::is(node, MESSAGES, "FolderIds", &[])?;
        Check::no_text(node)?;
        if node.children.is_empty() {
            return Err(invalid("FolderIds names at least one folder"));
        }
        let mut out = Vec::new();
        for one in &node.children {
            if one.is(TYPES, "DistinguishedFolderId") {
                Check::is(one, TYPES, "DistinguishedFolderId", &["Id", "ChangeKey"])?;
                Check::empty(one)?;
                let id = Check::attribute(one, "Id")?;
                if !DISTINGUISHED.contains(&id) {
                    return Err(invalid(format!("{id} is no distinguished folder")));
                }
                out.push(self.folder(id, true).cloned());
            } else if one.is(TYPES, "FolderId") {
                Check::is(one, TYPES, "FolderId", &["Id", "ChangeKey"])?;
                Check::empty(one)?;
                out.push(self.folder(Check::attribute(one, "Id")?, false).cloned());
            } else {
                return Err(invalid(format!("FolderIds has no place for {}", one.name)));
            }
        }
        Ok(out)
    }

    /// The answer to a request: the status, the response code it carries, and the body.
    fn answer(&mut self, asked: &mut Asked, request: &HttpRequest) -> Result<(u16, String, String), Refusal> {
        let root = xml::parse(request.body()).map_err(|_| invalid("the request is not XML that can be read"))?;
        Check::is(&root, SOAP, "Envelope", &[])?;
        let parts = Check::children(&root, SOAP, &["Header", "Body"])?;
        let header = Check::required(&root, parts.first().copied().flatten(), "Header")?;
        Check::is(header, SOAP, "Header", &[])?;
        Check::no_text(header)?;
        let version = header.child("RequestServerVersion").ok_or_else(|| invalid("a RequestServerVersion is needed"))?;
        Check::is(version, TYPES, "RequestServerVersion", &["Version"])?;
        Check::empty(version)?;
        if !Version::NAMES.contains(&Check::attribute(version, "Version")?) {
            return Err(Refusal { code: "ErrorInvalidServerVersion", why: "no such version of the schema".into() });
        }
        let body = Check::required(&root, parts.get(1).copied().flatten(), "Body")?;
        Check::is(body, SOAP, "Body", &[])?;
        Check::no_text(body)?;
        let [operation] = body.children.as_slice() else { return Err(invalid("the body holds one operation")) };
        asked.operation = operation.name.clone();
        if *operation.namespace != *MESSAGES {
            return Err(Refusal { code: "ErrorInvalidRequest", why: "the operation is not one of the service's".into() });
        }
        if self.busy_fault {
            self.busy_fault = false;
            // The fault of a busy server: `MessageXml` in the types' namespace.
            let body = envelope(&format!(
                "<s:Fault><faultcode xmlns:a=\"{TYPES}\">a:ErrorServerBusy</faultcode><faultstring>The server cannot service this request right now.</faultstring>\
                 <detail><e:ResponseCode xmlns:e=\"{ERRORS}\">ErrorServerBusy</e:ResponseCode><e:Message xmlns:e=\"{ERRORS}\">Busy</e:Message>\
                 <t:MessageXml xmlns:t=\"{TYPES}\"><t:Value Name=\"BackOffMilliseconds\">{}</t:Value></t:MessageXml></detail></s:Fault>",
                self.busy.1
            ));
            return Ok((500, "ErrorServerBusy".into(), body));
        }
        if self.busy.0 > 0 {
            self.busy.0 = self.busy.0.saturating_sub(1);
            let name = operation.name.clone();
            let (code, inner) = if self.busy_inner {
                ("ErrorInternalServerError", "<t:Value Name=\"InnerErrorResponseCode\">ErrorServerBusy</t:Value>")
            } else {
                ("ErrorServerBusy", "")
            };
            let message = format!(
                "<m:{name}ResponseMessage ResponseClass=\"Error\"><m:MessageText>The server cannot service this request right now.</m:MessageText>\
                 <m:ResponseCode>{code}</m:ResponseCode><m:DescriptiveLinkKey>0</m:DescriptiveLinkKey>\
                 <m:MessageXml>{inner}<t:Value Name=\"BackOffMilliseconds\">{}</t:Value></m:MessageXml></m:{name}ResponseMessage>",
                self.busy.1
            );
            return Ok((200, code.into(), response(&name, &message)));
        }
        match operation.name.as_str() {
            "GetFolder" => self.get_folder(operation),
            "SyncFolderHierarchy" => self.sync_hierarchy(operation),
            "SyncFolderItems" => self.sync_items(operation),
            "GetItem" => self.get_item(operation),
            name if crate::standin_write::HANDLED.contains(&name) => {
                self.writes.push(name.to_owned());
                self.write(name, operation)
            }
            name if WRITES.contains(&name) => {
                self.writes.push(name.to_owned());
                let message = error_message(name, "ErrorAccessDenied");
                Ok((200, "ErrorAccessDenied".into(), response(name, &message)))
            }
            _ => Err(Refusal { code: "ErrorInvalidRequest", why: "no such operation".into() }),
        }
    }

    fn get_folder(&self, operation: &Node) -> Result<(u16, String, String), Refusal> {
        Check::is(operation, MESSAGES, "GetFolder", &[])?;
        let parts = Check::children(operation, MESSAGES, &["FolderShape", "FolderIds"])?;
        let shape = Check::required(operation, parts.first().copied().flatten(), "FolderShape")?;
        Check::is(shape, MESSAGES, "FolderShape", &[])?;
        let inside = Check::children(shape, TYPES, &["BaseShape", "AdditionalProperties"])?;
        let base = Check::required(shape, inside.first().copied().flatten(), "BaseShape")?;
        Check::is(base, TYPES, "BaseShape", &[])?;
        Check::no_children(base)?;
        let base = base.text.trim();
        if !["IdOnly", "Default", "AllProperties"].contains(&base) {
            return Err(invalid("BaseShape is IdOnly, Default or AllProperties"));
        }
        if inside.get(1).copied().flatten().is_some() {
            return Err(Refusal { code: "ErrorInvalidRequest", why: "this stand-in answers shapes without added properties only".into() });
        }
        let ids = self.folder_ids(Check::required(operation, parts.get(1).copied().flatten(), "FolderIds")?)?;
        let skip = usize::from(self.hostile == Some(Hostile::TooFew));
        let (mut messages, mut first) = (String::new(), String::new());
        for found in ids.into_iter().skip(skip) {
            let code = match found {
                Some(folder) => {
                    let id = if self.hostile == Some(Hostile::BadId) { "x".repeat(2_000) } else { folder.id.clone() };
                    let mut xml = format!("<t:Folder><t:FolderId Id=\"{}\" ChangeKey=\"AQAAAA==\"/>", escape(&id));
                    if base != "IdOnly" {
                        xml.push_str(&format!(
                            "<t:DisplayName>{}</t:DisplayName><t:TotalCount>{}</t:TotalCount><t:ChildFolderCount>0</t:ChildFolderCount><t:UnreadCount>{}</t:UnreadCount>",
                            escape(&folder.name),
                            folder.total,
                            folder.unread
                        ));
                    }
                    xml.push_str("</t:Folder>");
                    messages.push_str(&format!(
                        "<m:GetFolderResponseMessage ResponseClass=\"Success\"><m:ResponseCode>NoError</m:ResponseCode><m:Folders>{xml}</m:Folders></m:GetFolderResponseMessage>"
                    ));
                    "NoError"
                }
                None => {
                    messages.push_str(&error_message("GetFolder", "ErrorFolderNotFound"));
                    "ErrorFolderNotFound"
                }
            };
            if first.is_empty() {
                first = code.to_owned();
            }
        }
        Ok((200, first, response("GetFolder", &messages)))
    }

    fn respond(&mut self, request: &HttpRequest) -> HttpResponse {
        let mut asked = Asked { body: String::from_utf8_lossy(request.body()).into_owned(), ..Asked::default() };
        let authorized = request.field("authorization") == Some(self.authorization.as_str());
        let response = if let Some(reason) = self.blocked.clone() {
            asked.status = 403;
            HttpResponse::new(403, Vec::new()).with_field("X-EWS-Policy-Reason", &reason)
        } else if !authorized {
            asked.status = 401;
            HttpResponse::new(401, Vec::new())
        } else {
            let (status, code, body) = match self.answer(&mut asked, request) {
                Ok(done) => done,
                Err(refusal) => (500, refusal.code.to_owned(), fault(refusal.code, &refusal.why)),
            };
            asked.code = code;
            let (status, body) = match self.hostile {
                Some(how) => misbehave(how, body),
                None => (status, body),
            };
            asked.status = status;
            HttpResponse::new(status, body.into_bytes())
        };
        self.requests.push(asked);
        response
    }
}

fn envelope(body: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?><s:Envelope xmlns:s=\"{SOAP}\"><s:Header>\
         <h:ServerVersionInfo MajorVersion=\"15\" MinorVersion=\"20\" MajorBuildNumber=\"9000\" MinorBuildNumber=\"10\" Version=\"V2018_01_08\" xmlns:h=\"{TYPES}\"/>\
         </s:Header><s:Body>{body}</s:Body></s:Envelope>"
    )
}

/// One response message that says no.
pub(crate) fn error_message(operation: &str, code: &str) -> String {
    format!(
        "<m:{operation}ResponseMessage ResponseClass=\"Error\"><m:MessageText>The request was refused.</m:MessageText>\
         <m:ResponseCode>{code}</m:ResponseCode><m:DescriptiveLinkKey>0</m:DescriptiveLinkKey><m:Folders/></m:{operation}ResponseMessage>"
    )
}

pub(crate) fn response(operation: &str, messages: &str) -> String {
    envelope(&format!(
        "<m:{operation}Response xmlns:m=\"{MESSAGES}\" xmlns:t=\"{TYPES}\"><m:ResponseMessages>{messages}</m:ResponseMessages></m:{operation}Response>"
    ))
}

fn fault(code: &str, why: &str) -> String {
    envelope(&format!(
        "<s:Fault><faultcode xmlns:a=\"{TYPES}\">a:{code}</faultcode><faultstring xml:lang=\"en-US\">{}</faultstring>\
         <detail><e:ResponseCode xmlns:e=\"{ERRORS}\">{code}</e:ResponseCode><e:Message xmlns:e=\"{ERRORS}\">{}</e:Message></detail></s:Fault>",
        escape(why),
        escape(why)
    ))
}

/// What a hostile stand-in answers instead: a status and a body.
fn misbehave(how: Hostile, honest: String) -> (u16, String) {
    match how {
        Hostile::Dtd => {
            let inner = honest.split_once("?>").map_or(honest.as_str(), |(_, rest)| rest);
            (200, format!("<?xml version=\"1.0\"?><!DOCTYPE s [<!ENTITY a \"aaaaaaaaaa\"><!ENTITY b \"&a;&a;&a;&a;&a;&a;&a;&a;\">]>{inner}"))
        }
        Hostile::CutShort => (200, honest.chars().take(honest.chars().count() / 2).collect()),
        Hostile::NotXml => (200, "<html><body><h1>Sign in</h1><p>Nothing of the service is here.</body></html>".into()),
        Hostile::Deep => (200, format!("{}{}", "<a>".repeat(20_000), "</a>".repeat(20_000))),
        Hostile::Huge => {
            (200, format!("<?xml version=\"1.0\"?><s:Envelope xmlns:s=\"{SOAP}\"><s:Body>{}</s:Body></s:Envelope>", "x".repeat(17 * 1024 * 1024)))
        }
        Hostile::Fault => {
            (500, envelope("<s:Fault><faultcode>s:Server</faultcode><faultstring>An internal server error occurred.</faultstring></s:Fault>"))
        }
        Hostile::TooFew | Hostile::BadId | Hostile::BadItems => (200, honest),
    }
}

/// Runs a future to completion by polling it with a waker that does nothing. The crate's futures
/// only wait on the exchange, and a stand-in is always ready, so a few polls suffice. It gives
/// up (with `None`) rather than wait for ever.
pub fn block_on<F: Future>(future: F) -> Option<F::Output> {
    use std::pin::pin;
    use std::task::{Context, Poll, Waker};
    let mut future = pin!(future);
    let mut context = Context::from_waker(Waker::noop());
    for _ in 0..1000 {
        if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
            return Some(output);
        }
    }
    None
}
