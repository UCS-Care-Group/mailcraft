// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! `GetUserConfiguration` of the options Outlook on the web keeps in a mailbox (#484): the
//! signature the owner wrote there, so that an Exchange server account has it too.
//!
//! Outlook on the web (OWA) keeps its options as one *user configuration* of the mailbox, named
//! `OWA.UserOptions`, in the root folder; `GetUserConfiguration` ([MS-OXWSUSRCFG], the operation
//! and its `Dictionary` property) reads it. The *names inside* that dictionary are in no protocol
//! specification. Their public counterpart is the documentation of the Exchange cmdlet
//! `Set-MailboxMessageConfiguration`, whose `SignatureHtml`, `SignatureText`, `AutoAddSignature`
//! and `AutoAddSignatureOnReply` are these options; in the dictionary Exchange Server 2019 keeps
//! them as `signaturehtml`, `signaturetext`, `autoaddsignature` and `autoaddsignatureonreply`, in
//! lower case like the dictionary's `timezone` (checked there: an entry written under those names
//! shows in Outlook on the web's own options reply as `SignatureHtml` and the rest, and one
//! written under the camel-case names does not). Nothing promises another build keeps them. So this reader asks for the dictionary and takes only those four entries, by
//! name (whatever the case), with their types checked, and says nothing when one is missing or of
//! another type. A mailbox whose owner
//! never saved a signature has no such entries, and one that has never had its options saved may
//! have no such configuration at all (`ErrorItemNotFound`): both are *no signature*, not errors.
//!
//! Every value is the server's, so it is held to a size and returned as it came; whoever takes it
//! in reads its HTML through the same reader as any other signature. A signature past the size,
//! the HTML or the text, is no signature: not a cut one, and not the other one alone (an HTML
//! signature with a logo in it is often the one past the size, and its text alone would then be
//! taken for the owner's signature).

use mailcraft_model::sync::EwsSignature;

use crate::error::{Error, ResponseCode};
use crate::folder::DistinguishedFolder;
use crate::reply::Reply;
use crate::soap::{Element, MESSAGES, TYPES};
use crate::xml::Node;

/// The request `GetUserConfiguration` for the options of Outlook on the web.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GetUserOptions;

impl GetUserOptions {
    /// The user configuration asked for.
    pub const NAME: &'static str = "OWA.UserOptions";

    /// The operation as the schema has it: the configuration's name with the folder it is in (the
    /// root), then the property wanted, its dictionary.
    pub fn element(&self) -> Element {
        let name = Element::new("m:UserConfigurationName")
            .attr("Name", Self::NAME)
            .child(Element::new("t:DistinguishedFolderId").attr("Id", DistinguishedFolder::Root.as_str()));
        Element::new("m:GetUserConfiguration").child(name).child(Element::new("m:UserConfigurationProperties").text("Dictionary"))
    }

    /// The signature the owner has saved, or `None` when there is none (no configuration, no
    /// signature entries, or one past the sizes taken). A busy server is [`Error::Busy`]; any other
    /// refusal is [`Error::Server`].
    pub fn answer(&self, reply: &Reply) -> Result<Option<EwsSignature>, Error> {
        let messages = reply.messages("GetUserConfiguration")?;
        let [message] = messages.as_slice() else { return Err(Error::Unexpected) };
        let node = match message.ok() {
            Ok(node) => node,
            Err(Error::Server(ResponseCode::ItemNotFound)) => return Ok(None),
            Err(other) => return Err(other),
        };
        let Some(dictionary) = node.child_in(MESSAGES, "UserConfiguration").and_then(|configuration| configuration.child_in(TYPES, "Dictionary"))
        else {
            return Ok(None);
        };
        Ok(signature_of(dictionary))
    }
}

/// How many dictionary entries are looked at: Outlook on the web has a few hundred options.
const MAX_ENTRIES: usize = 2_000;

/// The four entries of the dictionary, when any is a signature.
fn signature_of(dictionary: &Node) -> Option<EwsSignature> {
    let mut found = EwsSignature::default();
    // A text the XML reader left out of a value (past its size, or with a reference in it) reads as
    // empty: then that one is no signature to take.
    let mut cut = false;
    for entry in dictionary.children.iter().filter(|c| c.is(TYPES, "DictionaryEntry")).take(MAX_ENTRIES) {
        let (Some(key), Some(value)) = (entry.child_in(TYPES, "DictionaryKey"), entry.child_in(TYPES, "DictionaryValue")) else { continue };
        let kind = value.text_of("Type");
        let inner = value.child_in(TYPES, "Value");
        let text = inner.map_or("", |v| v.text.as_str());
        let words = inner.is_some_and(|v| v.text_cut);
        match (key.text_of("Value").to_ascii_lowercase().as_str(), kind) {
            ("signaturehtml", "String") => (found.html, cut) = (text.to_owned(), cut || words),
            ("signaturetext", "String") => (found.text, cut) = (text.to_owned(), cut || words),
            ("autoaddsignature", "Boolean") => found.on_new = text.trim().eq_ignore_ascii_case("true"),
            ("autoaddsignatureonreply", "Boolean") => found.on_reply = text.trim().eq_ignore_ascii_case("true"),
            _ => {}
        }
    }
    // (A signature past the sizes is none: cutting HTML in the middle leaves it broken.)
    (!cut && found.within_bounds() && !found.is_empty()).then_some(found)
}

#[cfg(test)]
#[path = "userconfig_tests.rs"]
mod tests;
