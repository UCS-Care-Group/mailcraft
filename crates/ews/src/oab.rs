// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The organisation's whole address list (#1393): the Offline Address Book an Exchange server
//! publishes for the mail programs of its people ([MS-OXWOAB] for where it is and how it is
//! listed, [MS-OXOAB] for the file, read by `mailcraft-oab`).
//!
//! Three requests, each a read, each to the account's own server and nowhere else:
//!
//! 1. Autodiscover (POX, [MS-OXDSCLI]) at the host of the account's Exchange Web Services address,
//!    for `OABUrl`, where the address book is published. An `OABUrl` on any other host is not
//!    followed: the sign-in goes to the account's server alone.
//! 2. `oab.xml` there, the table of contents ([MS-OXWOAB] 2.2.4.1): for the first address list,
//!    the name of its full file of version 4 (`Full`, `ver="32"`), its sequence number and size.
//! 3. That file, at most [`MAX_FILE`] bytes, unpacked and read by `mailcraft_oab` within its caps.
//!
//! Of each entry only what a person is reached by and told apart by is kept, as for a look-up
//! ([`crate::resolve`]): the name, the SMTP address, the kind of recipient, company, department,
//! job title, the business and mobile numbers and the office, and their S/MIME certificates
//! (`PidTagUserX509Certificate`, `PidTagAddressBookX509Certificate`; Exchange's older form of
//! its own is not read). Postal addresses, home numbers, notes and the rest are read (and
//! checked) but not kept. Nothing of it is logged.

use std::sync::Arc;

use mailcraft_model::sync::{DirectoryKind, DirectoryPerson, EwsAddressList, MAX_ADDRESS_LIST, MAX_PERSON_CERTIFICATES};
use mailcraft_oab::{Limits, Record, tag};

use crate::autodiscover::{ANSWER_SCHEMA, REQUEST_SCHEMA, RESPONSE_SCHEMA};
use crate::error::{Error, MAX_BACK_OFF_MS};
use crate::http::{HttpExchange, HttpRequest, HttpResponse, Method, RESPONSE_LIMIT, Secret};
use crate::xml;

/// The largest Autodiscover answer read.
const MAX_ANSWER: usize = 256 * 1024;
/// The largest table of contents read.
const MAX_MANIFEST: usize = 2 * 1024 * 1024;
/// The largest address book file downloaded: larger ones are not read.
pub const MAX_FILE: usize = RESPONSE_LIMIT;
/// The most bytes of certificates held for a whole address list.
pub const MAX_CERTIFICATE_BYTES: usize = 64 * 1024 * 1024;
/// The longest file name taken from the table of contents.
const MAX_FILE_NAME: usize = 128;
/// `ver` of a version 4 full details file in the table of contents.
const VERSION_4: &str = "32";
/// How long a busy server asks to be left alone when it doesn't say, in milliseconds.
const BUSY_MS: u32 = 5_000;

/// The scheme and authority of an address: `https://mail.example.org:444` of
/// `https://mail.example.org:444/EWS/Exchange.asmx`, in lower case.
pub fn origin(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    (!authority.is_empty() && !authority.contains('@')).then(|| format!("{scheme}://{authority}").to_ascii_lowercase())
}

/// The Autodiscover request to the account's own server, for where its address book is.
pub fn autodiscover_request(ews_url: &str, email: &str, authorization: &Secret) -> Result<HttpRequest, Error> {
    let origin = origin(ews_url).ok_or(Error::Unexpected)?;
    let body = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?><Autodiscover xmlns=\"{REQUEST_SCHEMA}\"><Request><EMailAddress>{}</EMailAddress><AcceptableResponseSchema>{RESPONSE_SCHEMA}</AcceptableResponseSchema></Request></Autodiscover>",
        xml::escape_text(email)
    );
    let fields = [("Authorization", authorization.expose()), ("Content-Type", "text/xml; charset=utf-8"), ("Accept", "text/xml")];
    Ok(HttpRequest::new(Method::Post, &format!("{origin}/autodiscover/autodiscover.xml"), &fields, body.into_bytes())?
        .with_response_limit(MAX_ANSWER))
}

/// Where the address book is, from an Autodiscover answer: the first `OABUrl` on the same server
/// as `ews_url`, ending in `/`. `None` when the answer names none there.
pub fn oab_url(body: &[u8], ews_url: &str) -> Result<Option<String>, Error> {
    let root = xml::parse(body).map_err(|_| Error::Malformed)?;
    if root.name != "Autodiscover" || *root.namespace != *ANSWER_SCHEMA {
        return Err(Error::Unexpected);
    }
    let account = root.find(&["Response", "Account"]).ok_or(Error::Unexpected)?;
    let here = origin(ews_url).ok_or(Error::Unexpected)?;
    let found = account
        .all("Protocol")
        .map(|protocol| protocol.text_of("OABUrl"))
        .filter(|url| !url.is_empty() && url.len() <= 2048 && !url.contains(['?', '#', '\\']) && !url.chars().any(char::is_whitespace))
        .find(|url| origin(url).as_deref() == Some(here.as_str()))
        .map(|url| if url.ends_with('/') { url.to_owned() } else { format!("{url}/") });
    Ok(found)
}

/// A GET of `url` with the sign-in.
fn get(url: &str, authorization: &Secret, limit: usize) -> Result<HttpRequest, Error> {
    Ok(HttpRequest::new(Method::Get, url, &[("Authorization", authorization.expose())], Vec::new())?.with_response_limit(limit))
}

/// The full file of the first address list, as the table of contents names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FullFile {
    /// Its file name, a plain name in the address book's folder.
    pub name: String,
    /// The address list's sequence number: one more for each generation.
    pub sequence: u32,
    /// Its size, packed, as the table of contents says it.
    pub size: usize,
}

/// A file name that stays in its folder: letters, digits, `.`, `-` and `_`, not starting with `.`.
fn plain_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_FILE_NAME
        && !name.starts_with('.')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

/// The version 4 full file of the first address list that has one, from `oab.xml`.
pub fn full_file(body: &[u8]) -> Result<Option<FullFile>, Error> {
    let root = xml::parse(body).map_err(|_| Error::Malformed)?;
    if root.name != "OAB" {
        return Err(Error::Unexpected);
    }
    let found = root.all("OAL").flat_map(|list| list.all("Full")).find_map(|full| {
        let name = full.text.trim();
        if full.attribute("ver") != Some(VERSION_4) || !plain_name(name) {
            return None;
        }
        let sequence = full.attribute("seq").and_then(|n| n.trim().parse().ok())?;
        let size = full.attribute("size").and_then(|n| n.trim().parse().ok()).unwrap_or(0);
        Some(FullFile { name: name.to_owned(), sequence, size })
    });
    Ok(found)
}

/// The body of an answer that says yes, else what it says.
fn judged(response: &HttpResponse) -> Result<&[u8], Error> {
    match response.status() {
        200 => Ok(response.body()),
        401 => Err(Error::Unauthorized),
        403 => Err(Error::Forbidden),
        429 | 503 => {
            let seconds: u32 = response.retry_after().and_then(|s| s.trim().parse().ok()).unwrap_or(BUSY_MS / 1000);
            Err(Error::Busy { back_off_ms: seconds.saturating_mul(1000).clamp(1000, MAX_BACK_OFF_MS) })
        }
        status => Err(Error::Status(status)),
    }
}

/// The person an entry of the address book is to the window, or `None` for one mail can't go to.
pub fn person(record: &Record) -> Option<DirectoryPerson> {
    let text = |id: u16| record.text(id).unwrap_or("").to_owned();
    let mut email = text(tag::SMTP_ADDRESS);
    if email.is_empty() {
        // The main address is the one written `SMTP:` in capitals.
        email = record.texts(tag::PROXY_ADDRESSES).iter().find_map(|a| a.strip_prefix("SMTP:")).unwrap_or("").to_owned();
    }
    // [MS-OXOABK] 2.2.3.11: 0 a user, 1 a distribution list, 2 a public folder, 5 a personal
    // distribution list, 6 a contact outside the organisation.
    let kind = match record.integer(tag::DISPLAY_TYPE) {
        Some(0) => DirectoryKind::Mailbox,
        Some(1 | 5) => DirectoryKind::List,
        Some(2) => DirectoryKind::PublicFolder,
        Some(6) => DirectoryKind::Contact,
        _ => DirectoryKind::Other,
    };
    DirectoryPerson {
        name: text(tag::DISPLAY_NAME),
        email,
        kind,
        company: text(tag::COMPANY),
        department: text(tag::DEPARTMENT),
        title: text(tag::TITLE),
        phone: text(tag::BUSINESS_PHONE),
        mobile: text(tag::MOBILE_PHONE),
        office: text(tag::OFFICE_LOCATION),
        certificates: [tag::USER_X509_CERTIFICATE, tag::ADDRESS_BOOK_X509_CERTIFICATE]
            .into_iter()
            .flat_map(|id| record.binaries(id))
            .take(MAX_PERSON_CERTIFICATES.saturating_mul(2))
            .map(|der| Arc::new(der.to_vec()))
            .collect(),
    }
    .checked()
}

/// The properties [`person`] reads: the rest of an entry is not kept.
fn wanted(property: u32) -> bool {
    let id = (property >> 16) as u16;
    [
        tag::SMTP_ADDRESS,
        tag::PROXY_ADDRESSES,
        tag::DISPLAY_TYPE,
        tag::DISPLAY_NAME,
        tag::COMPANY,
        tag::DEPARTMENT,
        tag::TITLE,
        tag::BUSINESS_PHONE,
        tag::MOBILE_PHONE,
        tag::OFFICE_LOCATION,
        tag::USER_X509_CERTIFICATE,
        tag::ADDRESS_BOOK_X509_CERTIFICATE,
    ]
    .contains(&id)
}

/// The address list of a compressed full file: its people, by name.
pub fn address_list(file: &[u8]) -> Result<EwsAddressList, mailcraft_oab::Error> {
    let limits = Limits { max_records: MAX_ADDRESS_LIST.saturating_mul(4), ..Limits::default() };
    let details = mailcraft_oab::read_details_keeping(&mailcraft_oab::decompress(file, &limits)?, &limits, wanted)?;
    let mut people: Vec<DirectoryPerson> = details.records.iter().filter_map(person).collect();
    let more = people.len() > MAX_ADDRESS_LIST;
    people.truncate(MAX_ADDRESS_LIST);
    // The certificates held, in all: past it, the people after have none.
    let mut room = MAX_CERTIFICATE_BYTES;
    for p in &mut people {
        let size = p.certificates.iter().map(|c| c.len()).fold(0usize, usize::saturating_add);
        match room.checked_sub(size) {
            Some(left) => room = left,
            None => {
                p.certificates.clear();
                room = 0;
            }
        }
    }
    people.sort_by_cached_key(|p| (p.name.to_lowercase(), p.email.to_lowercase()));
    let name = details.name().map(|n| mailcraft_model::sync::one_line(n, 256)).unwrap_or_default();
    Ok(EwsAddressList { name, sequence: details.sequence().unwrap_or(0), people, more })
}

/// What stopped a download of the address list.
#[derive(Debug)]
pub enum OabError {
    /// A request, or its answer.
    Ews(Error),
    /// The server publishes no address book for this mailbox (or not on its own host).
    NotPublished,
    /// The file is larger than [`MAX_FILE`].
    TooLarge,
    /// The file can't be read.
    File(mailcraft_oab::Error),
}

impl From<Error> for OabError {
    fn from(error: Error) -> Self {
        OabError::Ews(error)
    }
}

/// Downloads and reads the account's address list from its own server: the three requests of the
/// module, over `exchange`.
pub async fn fetch<E: HttpExchange>(exchange: &E, ews_url: &str, email: &str, authorization: &Secret) -> Result<EwsAddressList, OabError> {
    let answer = exchange.exchange(&autodiscover_request(ews_url, email, authorization)?).await.map_err(Error::from)?;
    let url = oab_url(judged(&answer)?, ews_url)?.ok_or(OabError::NotPublished)?;
    let manifest = exchange.exchange(&get(&format!("{url}oab.xml"), authorization, MAX_MANIFEST)?).await.map_err(Error::from)?;
    let full = full_file(judged(&manifest)?)?.ok_or(OabError::NotPublished)?;
    if full.size > MAX_FILE {
        return Err(OabError::TooLarge);
    }
    let file = exchange.exchange(&get(&format!("{url}{}", full.name), authorization, MAX_FILE)?).await.map_err(Error::from)?;
    let list = address_list(judged(&file)?).map_err(OabError::File)?;
    Ok(list)
}

#[cfg(test)]
#[path = "oab_tests.rs"]
mod oab_tests;
