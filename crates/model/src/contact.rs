// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! People.

use mailcraft_time::Date;
use serde::{Deserialize, Serialize};

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
}

impl Contact {
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
