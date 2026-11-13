// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What a domain publishes for mail programs: the autoconfig document Thunderbird reads
//! (`clientConfig` version 1.1), at `https://autoconfig.<domain>/mail/config-v1.1.xml` or
//! `https://<domain>/.well-known/autoconfig/mail/config-v1.1.xml`, and the same document for
//! a domain in Mozilla's ISP database (`https://autoconfig.thunderbird.net/v1.1/<domain>`).
//! Part of finding a domain's servers ([`discover`](crate::discover), #1063).
//!
//! The document is fetched over HTTPS alone, with the domain in the address and nothing else:
//! not the mailbox name (the `emailaddress` query the format allows for is never sent; the
//! user name comes from the document's placeholders instead), no cookie, no sign-in. It is
//! hostile, as anything from the network is: read as a stream by a reader that takes no DTD
//! and expands no entity, with the depth, the text of each field and the number of servers
//! capped, and the answer's size capped by the HTTP layer. Nothing but the servers is taken:
//! of `emailProvider`, each `incomingServer` of type `imap` and `outgoingServer` of type `smtp`,
//! with its `hostname`, `port`, `socketType` and `username`. A server without encryption
//! (`socketType` `plain`) is never taken; implicit TLS (`SSL`) is preferred over `STARTTLS`.

use std::time::Duration;

use mailcraft_model::{Security, Server};
use quick_xml::Reader;
use quick_xml::events::{BytesRef, BytesStart, Event};

use crate::NetError;
use crate::dns::domain_name;
use crate::http::{self, Url};
use crate::xml_text;

/// How long fetching one document may take: it is one of several tries, and most domains
/// have none.
pub(crate) const PATIENCE: Duration = Duration::from_secs(6);
/// Deepest the elements may nest, longest text kept of one field (bytes), and most servers of
/// a kind read.
const MAX_DEPTH: usize = 32;
const MAX_FIELD: usize = 512;
const MAX_SERVERS: usize = 16;

/// What a document says of a domain's servers, after the choice among those it lists.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ClientConfig {
    pub(crate) incoming: Option<Server>,
    pub(crate) outgoing: Option<Server>,
    /// The user name of the incoming server, as the document writes it, placeholders and all;
    /// empty when it says none, or names no incoming server that is taken. (An account has one
    /// user name, the one it reads its mail with: the outgoing server's is not read for it.)
    pub(crate) username: String,
}

/// The document isn't one this reads.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Malformed;

/// Fetch the document at `url` and read it. `Ok(None)` when there is none there (any status
/// but 200), or what is there is no document with a server in it. Connecting may take
/// `patience`, and so may the answer ([`PATIENCE`], or what is left of the search's time).
pub(crate) fn fetch(url: &str, patience: Duration) -> Result<Option<ClientConfig>, NetError> {
    let url = Url::parse(url)?;
    let answer = http::get(&url, &[("Accept", "application/xml, text/xml")], patience)?;
    if answer.status != 200 {
        return Ok(None);
    }
    Ok(read(&answer.body).ok().flatten())
}

/// The kind of element the reader is in, as far as it cares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tag {
    ClientConfig,
    EmailProvider,
    /// `incomingServer type="imap"`, `outgoingServer type="smtp"`: whether it is the incoming one.
    Server(bool),
    Hostname,
    Port,
    SocketType,
    Username,
    Other,
}

/// One server as the document lists it, fields as text.
#[derive(Default)]
struct Listed {
    hostname: String,
    port: String,
    socket_type: String,
    username: String,
}

impl Listed {
    /// The server, when it is a host at an encrypted port.
    fn server(&self, usual: (u16, u16)) -> Option<Server> {
        let host = domain_name(&self.hostname)?;
        let security = match self.socket_type.trim().to_ascii_uppercase().as_str() {
            "SSL" => Security::Tls,
            "STARTTLS" => Security::StartTls,
            _ => return None,
        };
        let port = match self.port.trim() {
            "" => {
                if security == Security::Tls {
                    usual.0
                } else {
                    usual.1
                }
            }
            digits => digits.parse::<u16>().ok().filter(|port| *port != 0)?,
        };
        Some(Server { host, port, security })
    }
}

struct Walk {
    stack: Vec<Tag>,
    /// The text of the field being read, and how deep its element is.
    field: Option<(String, usize)>,
    /// That text holds something that isn't taken (too long, a reference to who knows what).
    spoiled: bool,
    /// The server being read, and whether it is an incoming one.
    server: Option<(Listed, bool)>,
    incoming: Vec<Listed>,
    outgoing: Vec<Listed>,
}

impl Walk {
    fn open(&mut self, element: &BytesStart<'_>) -> Result<(), Malformed> {
        if self.stack.len() >= MAX_DEPTH {
            return Err(Malformed);
        }
        let local = element.local_name();
        let tag = match (self.stack.as_slice(), local.as_ref()) {
            ([], "clientConfig") => Tag::ClientConfig,
            ([Tag::ClientConfig], "emailProvider") => Tag::EmailProvider,
            ([Tag::ClientConfig, Tag::EmailProvider], "incomingServer") if kind_of(element) == "imap" => Tag::Server(true),
            ([Tag::ClientConfig, Tag::EmailProvider], "outgoingServer") if kind_of(element) == "smtp" => Tag::Server(false),
            ([.., Tag::Server(_)], "hostname") => Tag::Hostname,
            ([.., Tag::Server(_)], "port") => Tag::Port,
            ([.., Tag::Server(_)], "socketType") => Tag::SocketType,
            ([.., Tag::Server(_)], "username") => Tag::Username,
            _ => Tag::Other,
        };
        if let Tag::Server(incoming) = tag {
            self.server = Some((Listed::default(), incoming));
        }
        self.stack.push(tag);
        if matches!(tag, Tag::Hostname | Tag::Port | Tag::SocketType | Tag::Username) && self.field.is_none() {
            self.field = Some((String::new(), self.stack.len()));
            self.spoiled = false;
        }
        Ok(())
    }

    fn close(&mut self) -> Result<(), Malformed> {
        let depth = self.stack.len();
        let Some(tag) = self.stack.pop() else { return Err(Malformed) };
        if self.field.as_ref().is_some_and(|(_, at)| *at == depth) {
            let text = self.field.take().map(|(text, _)| text).unwrap_or_default();
            let text = if self.spoiled { String::new() } else { text.trim().to_string() };
            // (The first of a field is the one taken.)
            if let Some((server, _)) = &mut self.server {
                let field = match tag {
                    Tag::Hostname => Some(&mut server.hostname),
                    Tag::Port => Some(&mut server.port),
                    Tag::SocketType => Some(&mut server.socket_type),
                    Tag::Username => Some(&mut server.username),
                    _ => None,
                };
                if let Some(field) = field.filter(|field| field.is_empty()) {
                    *field = text;
                }
            }
        }
        if let Tag::Server(_) = tag
            && let Some((server, incoming)) = self.server.take()
        {
            let list = if incoming { &mut self.incoming } else { &mut self.outgoing };
            if list.len() < MAX_SERVERS {
                list.push(server);
            }
        }
        Ok(())
    }

    /// Text of the element that is innermost now.
    fn text(&mut self, text: &str) {
        xml_text::push(&mut self.field, &mut self.spoiled, self.stack.len(), text, MAX_FIELD);
    }

    /// A reference in the text: a character by its number, or one of XML's own five. Any other
    /// is not looked up anywhere, and the field it stands in is not taken.
    fn reference(&mut self, reference: &BytesRef<'_>) {
        xml_text::push_reference(&mut self.field, &mut self.spoiled, self.stack.len(), reference, MAX_FIELD);
    }
}

/// The `type` attribute of a server element, in lower case; empty without one.
fn kind_of(element: &BytesStart<'_>) -> String {
    match element.try_get_attribute("type") {
        Ok(Some(attribute)) => attribute.value.trim().to_ascii_lowercase(),
        _ => String::new(),
    }
}

/// The best of the servers listed: implicit TLS over STARTTLS, else the first listed; one
/// without encryption or without a host is passed over.
fn best(listed: &[Listed], usual: (u16, u16)) -> Option<(Server, &Listed)> {
    let servers: Vec<(Server, &Listed)> = listed.iter().filter_map(|listed| listed.server(usual).map(|server| (server, listed))).collect();
    servers.iter().find(|(server, _)| server.security == Security::Tls).or_else(|| servers.first()).cloned()
}

/// The servers in the document `xml`. `Ok(None)` when it names no server that can be used.
pub(crate) fn read(xml: &[u8]) -> Result<Option<ClientConfig>, Malformed> {
    let xml = String::from_utf8_lossy(xml);
    let mut reader = Reader::from_str(&xml);
    let config = reader.config_mut();
    (config.check_end_names, config.allow_unmatched_ends, config.expand_empty_elements) = (true, false, false);
    let mut walk = Walk { stack: Vec::new(), field: None, spoiled: false, server: None, incoming: Vec::new(), outgoing: Vec::new() };
    // Every event takes at least a byte of the document, which bounds the loop.
    for _ in 0..=xml.len() {
        match reader.read_event().map_err(|_| Malformed)? {
            Event::Start(element) => walk.open(&element)?,
            Event::Empty(element) => {
                walk.open(&element)?;
                walk.close()?;
            }
            Event::End(_) => walk.close()?,
            Event::Text(text) => walk.text(&text.xml10_content()),
            Event::CData(text) => walk.text(&text.xml10_content()),
            Event::GeneralRef(reference) => walk.reference(&reference),
            // A document that brings a DTD along is not one this reads.
            Event::DocType(_) => return Err(Malformed),
            Event::Comment(_) | Event::Decl(_) | Event::PI(_) => {}
            Event::Eof => break,
        }
    }
    if !walk.stack.is_empty() {
        return Err(Malformed);
    }
    let incoming = best(&walk.incoming, (993, 143));
    let outgoing = best(&walk.outgoing, (465, 587));
    if incoming.is_none() && outgoing.is_none() {
        return Ok(None);
    }
    // The user name is the incoming server's, and goes with it alone.
    let username = incoming.as_ref().map(|(_, listed)| listed.username.clone()).unwrap_or_default();
    Ok(Some(ClientConfig { incoming: incoming.map(|(server, _)| server), outgoing: outgoing.map(|(server, _)| server), username }))
}

#[cfg(test)]
#[path = "autoconfig_tests.rs"]
mod autoconfig_tests;
