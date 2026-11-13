// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! People.

use mailcraft_time::Date;
use serde::{Deserialize, Serialize};

use crate::remote::{ContactField, RemoteContact};
use crate::{Address, Id, Origin};

/// A value with a label (`work`, `home`, `mobile`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Labeled {
    pub label: String,
    pub value: String,
}

impl Labeled {
    pub fn new(label: impl Into<String>, value: impl Into<String>) -> Labeled {
        Labeled { label: label.into(), value: value.into() }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PostalAddress {
    pub label: String,
    pub street: String,
    pub city: String,
    pub region: String,
    pub postal_code: String,
    pub country: String,
}

impl PostalAddress {
    /// The address on one line, skipping empty parts.
    pub fn one_line(&self) -> String {
        let city = [self.city.as_str(), self.region.as_str(), self.postal_code.as_str()]
            .into_iter()
            .filter(|p| !p.trim().is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        [self.street.as_str(), city.as_str(), self.country.as_str()].into_iter().filter(|p| !p.trim().is_empty()).collect::<Vec<_>>().join(", ")
    }

    pub fn is_empty(&self) -> bool {
        self.one_line().is_empty()
    }
}

/// What a key on a contact's card is. In JSON its name: `x509`, `openPgp`, `uri`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum KeyKind {
    /// An S/MIME certificate (X.509, DER).
    #[default]
    X509,
    /// An OpenPGP public key, binary or ASCII-armoured, as the card had it.
    OpenPgp,
    /// Where the card says a key is ([`ContactKey::uri`]); never fetched.
    Uri,
    /// A kind written by a later version, by the name it was written under (at most
    /// [`KeyKind::MAX_NAME`] bytes), so that saving here keeps it; never written to a vCard.
    Unknown(String),
}

impl KeyKind {
    /// Longest name of an unknown kind kept, in bytes.
    pub const MAX_NAME: usize = 64;

    /// The kind's name in JSON.
    pub fn name(&self) -> &str {
        match self {
            KeyKind::X509 => "x509",
            KeyKind::OpenPgp => "openPgp",
            KeyKind::Uri => "uri",
            KeyKind::Unknown(name) => name,
        }
    }

    /// The kind a name gives; a name this version doesn't know is kept as it is, cut to
    /// [`KeyKind::MAX_NAME`] bytes on a character boundary.
    pub fn named(name: &str) -> KeyKind {
        match name {
            "x509" => KeyKind::X509,
            "openPgp" => KeyKind::OpenPgp,
            "uri" => KeyKind::Uri,
            other => {
                let mut end = other.len().min(KeyKind::MAX_NAME);
                while end > 0 && !other.is_char_boundary(end) {
                    end -= 1;
                }
                KeyKind::Unknown(other.get(..end).unwrap_or("").to_string())
            }
        }
    }
}

impl Serialize for KeyKind {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.name())
    }
}

impl<'de> Deserialize<'de> for KeyKind {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<KeyKind, D::Error> {
        let name = String::deserialize(d)?;
        Ok(KeyKind::named(&name))
    }
}

/// A public key or certificate a contact's card carries (vCard `KEY`). The vCard reader keeps
/// only what has the shape of a certificate or of an OpenPGP public key, never secret key
/// material; nothing here checks a signature or trusts it: these are bytes that travel with the
/// contact through import and export.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ContactKey {
    pub kind: KeyKind,
    /// The key's bytes; empty for a [`KeyKind::Uri`]. Base64 in JSON.
    #[serde(with = "key_bytes", skip_serializing_if = "Vec::is_empty")]
    pub data: Vec<u8>,
    /// For a [`KeyKind::Uri`]: the address the card gave (`https:`, `ldap:`, `openpgp4fpr:`).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub uri: String,
    /// For a [`KeyKind::Uri`]: what the card says is there, [`ContactKey::X509_MEDIA_TYPE`] or
    /// [`ContactKey::OPENPGP_MEDIA_TYPE`]; empty when it doesn't say.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub media_type: String,
}

impl ContactKey {
    /// Most bytes of one key kept.
    pub const MAX_BYTES: usize = 64 * 1024;
    /// Longest address of a key kept, in bytes.
    pub const MAX_URI: usize = 2048;
    /// The media type of an S/MIME certificate.
    pub const X509_MEDIA_TYPE: &str = "application/pkix-cert";
    /// The media type of OpenPGP keys.
    pub const OPENPGP_MEDIA_TYPE: &str = "application/pgp-keys";
}

/// A key's bytes as base64 text in JSON.
mod key_bytes {
    use serde::{Deserialize as _, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&mailcraft_charset::base64::STANDARD.encode(v))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(d)?;
        mailcraft_charset::base64::STANDARD.decode(text.trim()).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Contact {
    pub id: Id,
    /// The name lists show; when empty, [`Contact::name`] builds one.
    pub display_name: String,
    pub first: String,
    pub middle: String,
    pub last: String,
    pub nickname: String,
    pub company: String,
    pub title: String,
    pub department: String,
    pub emails: Vec<Labeled>,
    pub phones: Vec<Labeled>,
    pub addresses: Vec<PostalAddress>,
    pub websites: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub birthday: Option<Date>,
    pub notes: String,
    pub categories: Vec<String>,
    pub favorite: bool,
    /// The S/MIME certificates and OpenPGP keys the contact's card carried (vCard `KEY`), at most
    /// [`Contact::MAX_KEYS`]: written back on export, kept by an edit and a merge, not searched.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub keys: Vec<ContactKey>,
    /// vCard `UID`, kept so an exported card updates the same contact elsewhere.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub uid: String,
    /// Where in imported .pst files the contact was, each as the file's identity and the item's
    /// node id, separated by spaces (a merge keeps those of every contact it joins): how a second
    /// import of the file knows it is here already. Private: never shown,
    /// exported or handed to an agent.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub imported: String,
    /// The name of the .pst file the contact was imported from (empty for any other). Shown on
    /// the contact's card, found by a search for `is:imported`, kept by an edit and a merge, and
    /// never set by a command's params. Not written to a vCard.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub imported_from: String,
    /// Set for a contact that an account's server keeps: it is a copy of the server's, and what
    /// the server calls it is how the next download finds it again.
    #[serde(skip_serializing_if = "Origin::is_local")]
    pub origin: Origin,
    /// For a contact of an account whose contacts are written to its server
    /// ([`crate::Account::writes_contacts`]): the contact as the server last said it, or as it was
    /// last told. What was changed here since is the difference ([`Contact::changed_fields`]), and
    /// only that is sent. Never set for a contact that lives on this computer only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub synced: Option<Box<RemoteContact>>,
}

/// A contact that was deleted here and that its account's server is yet to be told about.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ContactGone {
    pub account: Id,
    /// What the server calls it.
    pub id: String,
}

/// Most deletions of contacts that wait for a server at once; the rest are forgotten.
pub const MAX_CONTACT_GONE: usize = 5_000;

impl Contact {
    /// The contact as a server would have it: the inverse of what a download makes of a server's
    /// contact, cut to size as everything that crosses to a server is ([`RemoteContact::tidy`]).
    /// Its id and change key are the server's own ([`Contact::origin`]); what is only kept here
    /// (categories, the favourite mark, keys) is not part of it.
    pub fn as_remote(&self) -> RemoteContact {
        let mut card = RemoteContact {
            id: self.origin.id.clone(),
            change_key: self.origin.change_key.clone(),
            display_name: self.display_name.clone(),
            first: self.first.clone(),
            middle: self.middle.clone(),
            last: self.last.clone(),
            nickname: self.nickname.clone(),
            company: self.company.clone(),
            title: self.title.clone(),
            department: self.department.clone(),
            emails: self.emails.clone(),
            phones: self.phones.clone(),
            addresses: self.addresses.clone(),
            websites: self.websites.clone(),
            birthday: self.birthday,
            notes: self.notes.clone(),
            modified: 0,
        };
        card.tidy();
        card
    }

    /// Whether every part a server keeps is, as it stands here, just what `card` has: nothing is
    /// copied to find out.
    fn same_as(&self, card: &RemoteContact) -> bool {
        self.display_name == card.display_name
            && self.first == card.first
            && self.middle == card.middle
            && self.last == card.last
            && self.nickname == card.nickname
            && self.company == card.company
            && self.title == card.title
            && self.department == card.department
            && self.emails == card.emails
            && self.phones == card.phones
            && self.addresses == card.addresses
            && self.websites == card.websites
            && self.birthday == card.birthday
            && self.notes == card.notes
    }

    /// The parts that were changed here since the server last had the contact
    /// ([`Contact::synced`]), in the order of [`ContactField::ALL`]. None for a contact that has
    /// no such memory: one on this computer only, or one the server is yet to be given.
    pub fn changed_fields(&self) -> Vec<ContactField> {
        match self.synced.as_deref() {
            // (What a server is sent is cut to size first, and what it said was cut to size when
            // it came: a contact that is the same before the cut is the same after it.)
            Some(base) if !self.same_as(base) => ContactField::changed(base, &self.as_remote()),
            _ => Vec::new(),
        }
    }

    /// Put the part `field` of `from` into the contact.
    pub fn set_part(&mut self, field: ContactField, from: &RemoteContact) {
        match field {
            ContactField::Name => {
                (self.display_name, self.first) = (from.display_name.clone(), from.first.clone());
                (self.middle, self.last) = (from.middle.clone(), from.last.clone());
            }
            ContactField::Nickname => self.nickname = from.nickname.clone(),
            ContactField::Organization => (self.company, self.department) = (from.company.clone(), from.department.clone()),
            ContactField::Title => self.title = from.title.clone(),
            ContactField::Emails => self.emails = from.emails.clone(),
            ContactField::Phones => self.phones = from.phones.clone(),
            ContactField::Addresses => self.addresses = from.addresses.clone(),
            ContactField::Websites => self.websites = from.websites.clone(),
            ContactField::Birthday => self.birthday = from.birthday,
            ContactField::Notes => self.notes = from.notes.clone(),
        }
    }

    /// Most keys kept for one contact.
    pub const MAX_KEYS: usize = 32;

    /// The name to show: the display name, else first and last, the company, or the first address.
    pub fn name(&self) -> String {
        if !self.display_name.trim().is_empty() {
            return self.display_name.trim().to_string();
        }
        let full = [self.first.trim(), self.middle.trim(), self.last.trim()].into_iter().filter(|p| !p.is_empty()).collect::<Vec<_>>().join(" ");
        if !full.is_empty() {
            return full;
        }
        if !self.company.trim().is_empty() {
            return self.company.trim().to_string();
        }
        self.emails.first().map(|e| e.value.clone()).unwrap_or_default()
    }

    /// `Last, First` for sorting by surname.
    pub fn file_as(&self) -> String {
        match (self.last.trim(), self.first.trim()) {
            ("", _) => self.name(),
            (last, "") => last.to_string(),
            (last, first) => format!("{last}, {first}"),
        }
    }

    pub fn primary_email(&self) -> Option<&str> {
        self.emails.iter().map(|e| e.value.as_str()).find(|e| !e.trim().is_empty())
    }

    /// The contact as a recipient, when it has an address.
    pub fn address(&self) -> Option<Address> {
        self.primary_email().map(|e| Address::new(self.name(), e))
    }

    pub fn has_email(&self, email: &str) -> bool {
        self.emails.iter().any(|e| e.value.trim().eq_ignore_ascii_case(email.trim()))
    }

    /// Whether any name, company, address or phone contains `needle` (case-insensitive). The file
    /// a contact was imported from is not text to find it by: only `is:imported` finds those.
    pub fn matches(&self, needle: &str) -> bool {
        let mut n = needle.trim();
        // `is:imported`, alone or before other words ("is:imported smith"): imported ones only.
        // A loop, not a call of itself: a query of the term many times over can't use up the stack.
        while let Some(rest) = n
            .get(..11)
            .filter(|t| t.eq_ignore_ascii_case("is:imported"))
            .and_then(|_| n.get(11..))
            .filter(|r| r.is_empty() || r.starts_with(char::is_whitespace))
        {
            if self.imported_from.is_empty() {
                return false;
            }
            n = rest.trim();
        }
        n.is_empty()
            || [&self.display_name, &self.first, &self.middle, &self.last, &self.nickname, &self.company, &self.title, &self.department, &self.notes]
                .into_iter()
                .any(|f| crate::contains_ci(f, n))
            || self.emails.iter().chain(&self.phones).any(|l| crate::contains_ci(&l.value, n))
            || self.categories.iter().any(|c| crate::contains_ci(c, n))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_fall_back() {
        let mut c = Contact { first: "Ada".into(), last: "Lovelace".into(), ..Default::default() };
        assert_eq!((c.name().as_str(), c.file_as().as_str()), ("Ada Lovelace", "Lovelace, Ada"));
        c.display_name = "Countess of Lovelace".into();
        assert_eq!(c.name(), "Countess of Lovelace");
        let org = Contact { company: "Analytical Engines Ltd".into(), ..Default::default() };
        assert_eq!((org.name().as_str(), org.file_as().as_str()), ("Analytical Engines Ltd", "Analytical Engines Ltd"));
        let mail = Contact { emails: vec![Labeled::new("work", "x@example.org")], ..Default::default() };
        assert_eq!(mail.name(), "x@example.org");
        assert_eq!(Contact::default().name(), "");
    }

    #[test]
    fn addresses_and_search() {
        let c = Contact {
            first: "Grace".into(),
            last: "Hopper".into(),
            emails: vec![Labeled::new("work", ""), Labeled::new("home", "grace@example.org")],
            phones: vec![Labeled::new("mobile", "+1 555 0100")],
            addresses: vec![PostalAddress {
                street: "1 Navy Way".into(),
                city: "Arlington".into(),
                region: "VA".into(),
                postal_code: "22201".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        assert_eq!(c.address(), Some(Address::new("Grace Hopper", "grace@example.org")));
        assert!(c.has_email(" GRACE@example.org"));
        assert!(c.matches("hopp") && c.matches("555 01") && c.matches("") && !c.matches("lovelace"));
        assert_eq!(c.addresses[0].one_line(), "1 Navy Way, Arlington VA 22201");
        assert!(PostalAddress::default().is_empty());
    }
}
