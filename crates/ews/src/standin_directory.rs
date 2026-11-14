// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The stand-in's address list (#1262): `ResolveNames` over [`State::directory`], as
//! [MS-OXWSRSLNM] documents it. A text matches a person whose name has a word that starts with
//! it, or whose address does; nobody is `ErrorNameResolutionNoResults` (an error), one person
//! `NoError`, several the warning `ErrorNameResolutionMultipleResults`, at most 100 of them with
//! `TotalItemsInView` saying how many there were.

use crate::soap::MESSAGES;
use crate::standin::{Check, Refusal, State, invalid, response};
use crate::xml::{Node, escape};

/// A person (or list) in the stand-in's address list.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Person {
    pub name: String,
    pub email: String,
    /// The `MailboxType` it is answered with (`Mailbox`, `PublicDL`, `Contact`).
    pub kind: String,
    pub company: String,
    pub department: String,
    pub title: String,
    /// Its business phone number.
    pub phone: String,
    /// Its mobile phone number.
    pub mobile: String,
    pub office: String,
    /// Written into its `Contact` as a home phone number, which a client is not to keep.
    pub home: String,
}

impl Person {
    /// A person's mailbox of this name and address.
    pub fn new(name: &str, email: &str) -> Self {
        Self { name: name.into(), email: email.into(), kind: "Mailbox".into(), ..Default::default() }
    }
}

/// The most people one answer lists, as Exchange's.
const MOST: usize = 100;

impl State {
    pub(crate) fn resolve_names(&self, operation: &Node) -> Result<(u16, String, String), Refusal> {
        Check::is(operation, MESSAGES, "ResolveNames", &["ReturnFullContactData", "SearchScope", "ContactDataShape"])?;
        Check::attribute(operation, "ReturnFullContactData")?;
        let scope = operation.attribute("SearchScope").unwrap_or("ActiveDirectoryContacts");
        if !matches!(scope, "ActiveDirectory" | "ActiveDirectoryContacts" | "Contacts" | "ContactsActiveDirectory") {
            return Err(invalid("SearchScope is not one of the schema's"));
        }
        let parts = Check::children(operation, MESSAGES, &["ParentFolderIds", "UnresolvedEntry"])?;
        let entry = Check::required(operation, parts.get(1).copied().flatten(), "UnresolvedEntry")?;
        Check::is(entry, MESSAGES, "UnresolvedEntry", &[])?;
        let text = entry.text.trim().to_lowercase();
        if text.is_empty() {
            return Err(invalid("UnresolvedEntry is empty"));
        }
        let full = operation.attribute("ReturnFullContactData") == Some("true");
        // (Only the address list is searched with `ActiveDirectory`: this stand-in's mailbox has no
        // contacts that another scope would add.)
        let found: Vec<&Person> = self
            .directory
            .iter()
            .filter(|p| p.name.to_lowercase().split_whitespace().any(|word| word.starts_with(&text)) || p.email.to_lowercase().starts_with(&text))
            .collect();
        if found.is_empty() {
            let message = "<m:ResolveNamesResponseMessage ResponseClass=\"Error\"><m:MessageText>No results were found.</m:MessageText>\
                 <m:ResponseCode>ErrorNameResolutionNoResults</m:ResponseCode><m:DescriptiveLinkKey>0</m:DescriptiveLinkKey></m:ResolveNamesResponseMessage>";
            return Ok((200, "ErrorNameResolutionNoResults".into(), response("ResolveNames", message)));
        }
        let listed: String = found.iter().take(MOST).map(|p| resolution(p, full)).collect();
        let (class, code) = if found.len() == 1 { ("Success", "NoError") } else { ("Warning", "ErrorNameResolutionMultipleResults") };
        let text_line = if class == "Warning" { "<m:MessageText>Multiple results were found.</m:MessageText>" } else { "" };
        let message = format!(
            "<m:ResolveNamesResponseMessage ResponseClass=\"{class}\">{text_line}<m:ResponseCode>{code}</m:ResponseCode>\
             <m:ResolutionSet TotalItemsInView=\"{total}\" IncludesLastItemInRange=\"{last}\">{listed}</m:ResolutionSet></m:ResolveNamesResponseMessage>",
            total = found.len(),
            last = found.len() <= MOST,
        );
        Ok((200, code.into(), response("ResolveNames", &message)))
    }
}

fn resolution(p: &Person, full: bool) -> String {
    let mailbox = format!(
        "<t:Mailbox><t:Name>{}</t:Name><t:EmailAddress>{}</t:EmailAddress><t:RoutingType>SMTP</t:RoutingType><t:MailboxType>{}</t:MailboxType></t:Mailbox>",
        escape(&p.name),
        escape(&p.email),
        escape(&p.kind)
    );
    let contact = if full {
        let field = |name: &str, value: &str| if value.is_empty() { String::new() } else { format!("<t:{name}>{}</t:{name}>", escape(value)) };
        format!(
            "<t:Contact>{}{}{}{}{}{}</t:Contact>",
            field("DisplayName", &p.name),
            field("CompanyName", &p.company),
            phones(p),
            field("Department", &p.department),
            field("JobTitle", &p.title),
            field("OfficeLocation", &p.office),
        )
    } else {
        String::new()
    };
    format!("<t:Resolution>{mailbox}{contact}</t:Resolution>")
}

/// Its `PhoneNumbers`, in the schema's form; none when it has no number.
fn phones(p: &Person) -> String {
    let entries: String = [("BusinessPhone", &p.phone), ("HomePhone", &p.home), ("MobilePhone", &p.mobile)]
        .iter()
        .filter(|(_, number)| !number.is_empty())
        .map(|(key, number)| format!("<t:Entry Key=\"{key}\">{}</t:Entry>", escape(number)))
        .collect();
    if entries.is_empty() { String::new() } else { format!("<t:PhoneNumbers>{entries}</t:PhoneNumbers>") }
}
