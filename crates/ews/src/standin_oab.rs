// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The stand-in's Offline Address Book (#1393), next to its Exchange Web Services, at the same
//! host and behind the same sign-in, as Exchange publishes it:
//!
//! - `POST /autodiscover/autodiscover.xml` answers the account's settings, with `OABUrl`;
//! - `GET <OABUrl>oab.xml` lists one address list with one version 4 full file;
//! - `GET <OABUrl><file>` is that file, made of [`State::directory`] (its blocks stored).
//!
//! It runs inside the test: nothing here leaves the machine.

use mailcraft_oab::test_writer::{details, stored};
use mailcraft_oab::{PropDef, Record, Value};

use crate::autodiscover::{ANSWER_SCHEMA, RESPONSE_SCHEMA};
use crate::http::{HttpRequest, HttpResponse, Method};
use crate::oab::origin;
use crate::standin::State;
use crate::xml::escape_text;

/// Where the address book is published, under the host.
pub const PATH: &str = "/OAB/5a0c1e22-0b6f-4d3e-9c1a-00000000c0de/";
/// The name of its full file.
pub const FILE: &str = "5a0c1e22-0b6f-4d3e-9c1a-00000000c0de-data-7.lzx";

/// How the stand-in publishes its Offline Address Book.
#[derive(Clone, Debug)]
pub struct Publishing {
    /// It publishes one at all (Autodiscover names no `OABUrl` otherwise).
    pub published: bool,
    /// Its `OABUrl` names this host instead of the stand-in's own (`https://elsewhere.example`).
    pub elsewhere: Option<String>,
    /// The sequence number of the address list.
    pub sequence: u32,
}

impl Default for Publishing {
    fn default() -> Self {
        Self { published: true, elsewhere: None, sequence: 7 }
    }
}

/// The schema of the stand-in's entries: what Exchange writes, in part.
fn schema() -> Vec<PropDef> {
    let def = |tag: u32| PropDef { tag, primary_key: false, anr: false };
    vec![
        PropDef { tag: 0x3003_001E, primary_key: true, anr: false },
        PropDef { tag: 0x39FE_001F, primary_key: true, anr: false },
        def(0x3A19_001F),
        PropDef { tag: 0x800F_101F, primary_key: false, anr: true },
        PropDef { tag: 0x3001_001F, primary_key: false, anr: true },
        def(0x3A18_001F),
        def(0x3A16_001F),
        def(0x3A17_001F),
        def(0x3A1C_001F),
        def(0x3A09_001F),
        def(0x3A08_001F),
        def(0x0FFE_0003),
        def(0x3900_0003),
    ]
}

impl State {
    /// The full file of the address book.
    pub fn oab_file(&self) -> Vec<u8> {
        let text = |tag: u32, value: &str| (!value.is_empty()).then(|| (tag, Value::Text(value.to_owned())));
        let header_schema = [0x6800_001F, 0x6804_001E, 0x6801_0003, 0x6802_001E].map(|tag| PropDef { tag, primary_key: false, anr: false });
        let header = Record {
            values: vec![
                (0x6800_001F, Value::Text("\\Global Address List".into())),
                (0x6804_001E, Value::Text("/".into())),
                (0x6801_0003, Value::Integer(self.oab.sequence)),
                (0x6802_001E, Value::Text("5a0c1e22-0b6f-4d3e-9c1a-00000000c0de".into())),
            ],
        };
        let records: Vec<Record> = self
            .directory
            .iter()
            .map(|p| {
                let display_type = match p.kind.as_str() {
                    "PublicDL" | "PrivateDL" => 1,
                    "PublicFolder" => 2,
                    "Contact" => 6,
                    _ => 0,
                };
                let dn = format!("/o=Example/ou=Exchange Administrative Group/cn=Recipients/cn={}", p.email.split('@').next().unwrap_or(""));
                let values = [
                    Some((0x3003_001E, Value::Text(dn))),
                    text(0x39FE_001F, &p.email),
                    text(0x3A19_001F, &p.office),
                    Some((0x800F_101F, Value::Texts(vec![format!("SMTP:{}", p.email), format!("X500:/o=Example/cn={}", p.name)]))),
                    text(0x3001_001F, &p.name),
                    text(0x3A18_001F, &p.department),
                    text(0x3A16_001F, &p.company),
                    text(0x3A17_001F, &p.title),
                    text(0x3A1C_001F, &p.mobile),
                    text(0x3A09_001F, &p.home),
                    text(0x3A08_001F, &p.phone),
                    Some((0x0FFE_0003, Value::Integer(if display_type == 1 { 8 } else { 6 }))),
                    Some((0x3900_0003, Value::Integer(display_type))),
                ];
                Record { values: values.into_iter().flatten().collect() }
            })
            .collect();
        stored(&details(&header_schema, &schema(), &header, &records), 0x40000)
    }

    /// The answer to a request for the address book, when it is one: what it was, and the answer.
    pub(crate) fn offline_address_book(&self, request: &HttpRequest) -> Option<(&'static str, HttpResponse)> {
        let host = origin(request.url())?;
        let path = request.url().get(host.len()..).unwrap_or("");
        let not_found = || HttpResponse::new(404, Vec::new());
        match (request.method(), path) {
            (Method::Post, "/autodiscover/autodiscover.xml") => {
                let asked = String::from_utf8_lossy(request.body());
                if !asked.contains("<EMailAddress>") || !asked.contains(RESPONSE_SCHEMA) {
                    return Some(("Autodiscover", HttpResponse::new(400, Vec::new())));
                }
                let oab = match (&self.oab.elsewhere, self.oab.published) {
                    (_, false) => String::new(),
                    (Some(other), true) => format!("<OABUrl>{}</OABUrl>", escape_text(&format!("{other}{PATH}"))),
                    (None, true) => format!("<OABUrl>{}</OABUrl>", escape_text(&format!("{host}{PATH}"))),
                };
                let body = format!(
                    "<?xml version=\"1.0\" encoding=\"utf-8\"?><Autodiscover xmlns=\"{ANSWER_SCHEMA}\"><Response xmlns=\"{RESPONSE_SCHEMA}\">\
                     <User><DisplayName>Stand-in</DisplayName></User><Account><AccountType>email</AccountType><Action>settings</Action>\
                     <Protocol><Type>EXCH</Type><Server>exchange.example</Server><EwsUrl>{0}/EWS/Exchange.asmx</EwsUrl>{oab}</Protocol>\
                     <Protocol><Type>WEB</Type></Protocol></Account></Response></Autodiscover>",
                    escape_text(&host)
                );
                Some(("Autodiscover", HttpResponse::new(200, body.into_bytes())))
            }
            (Method::Get, rest) if rest.starts_with(PATH) => {
                if !self.oab.published || self.oab.elsewhere.is_some() {
                    return Some(("OAB", not_found()));
                }
                match rest.get(PATH.len()..).unwrap_or("") {
                    "oab.xml" => {
                        let file = self.oab_file();
                        let body = format!(
                            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\r\n<OAB>\r\n<OAL id=\"5a0c1e22-0b6f-4d3e-9c1a-00000000c0de\" dn=\"/\" name=\"\\Global Address List\">\
                             <Full seq=\"{0}\" ver=\"32\" size=\"{1}\" uncompressedsize=\"{1}\" SHA=\"0000\">{FILE}</Full>\
                             <Template seq=\"1\" ver=\"7\" size=\"10\" uncompressedsize=\"10\" SHA=\"0000\" langid=\"0409\" type=\"windows\">templ-1.lzx</Template>\
                             </OAL>\r\n</OAB>",
                            self.oab.sequence,
                            file.len()
                        );
                        Some(("oab.xml", HttpResponse::new(200, body.into_bytes())))
                    }
                    FILE => Some(("OAB file", HttpResponse::new(200, self.oab_file()))),
                    _ => Some(("OAB", not_found())),
                }
            }
            _ => None,
        }
    }
}
