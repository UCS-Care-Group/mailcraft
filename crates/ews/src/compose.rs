// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Drafts and sending (#252, E6): `CreateItem` with `SaveOnly` into the server's Drafts, `FindItem`
//! by MailCraft's marker (a draft whose make was not answered is found, not made twice),
//! `CreateAttachment` and `DeleteAttachment`, and `CreateItem` with `SendAndSaveCopy` for every
//! message sent (a saved draft too: `SendItem` of a draft files no copy on some servers) ([MS-OXWSCORE], [MS-OXWSMSG], [MS-OXWSATT]).
//!
//! **The marker** is an extended property of the draft (a string in the `PublicStrings` set called
//! `MailCraftMarker`), which no mail program shows. **Nothing here is sent twice**: the host marks
//! these requests as ones that are never repeated ([`crate::Client::call_once`]).

use mailcraft_mime::WriteOptions;
use mailcraft_model::sync::AttachmentUp;
use mailcraft_model::{Address, Importance, Message};

use crate::basic::base64;
use crate::changes::{Saved, id_element, line, recipients};
use crate::error::Error;
use crate::folder::valid_id;
use crate::reply::Reply;
use crate::soap::{Element, MESSAGES, TYPES};
use crate::xml::Node;

/// The name of the marker property.
const MARKER: &str = "MailCraftMarker";
/// The longest message sent as one MIME document (Exchange takes less by default).
const MAX_MIME: usize = 64 * 1024 * 1024;

/// Why a message cannot be made into a request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unsendable {
    /// No recipient.
    NoRecipients,
    /// An address mail cannot be sent to.
    BadAddress,
    /// Larger than any server takes.
    TooLarge,
    /// To be Digitally signed, and not (#30 R19): it is never sent unsigned.
    NotSigned,
    /// To be Encrypted, and not (#30 part 7): it is never sent in the clear.
    NotEncrypted,
}

impl std::fmt::Display for Unsendable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NoRecipients => "the message has no recipients",
            Self::BadAddress => "a recipient's address isn't one mail can be sent to",
            Self::TooLarge => "the message is too large to send",
            Self::NotSigned => mailcraft_mime::NOT_SIGNED,
            Self::NotEncrypted => mailcraft_mime::NOT_ENCRYPTED,
        })
    }
}

/// Where a folder is named: by its distinguished name, or by id.
fn folder_ref(holder: &str, distinguished: &str, id: Option<&str>) -> Element {
    let inner = match id {
        Some(id) if !id.is_empty() => id_element("t:FolderId", id, None),
        _ => Element::new("t:DistinguishedFolderId").attr("Id", distinguished),
    };
    Element::new(holder).child(inner)
}

fn marker_uri() -> Element {
    Element::new("t:ExtendedFieldURI").attr("DistinguishedPropertySetId", "PublicStrings").attr("PropertyName", MARKER).attr("PropertyType", "String")
}

fn file_attachment(a: &AttachmentUp) -> Element {
    let mut f = Element::new("t:FileAttachment").child(Element::new("t:Name").text(&line(&a.name)));
    if !a.mime.is_empty() {
        f = f.child(Element::new("t:ContentType").text(&a.mime));
    }
    if !a.content_id.is_empty() {
        f = f.child(Element::new("t:ContentId").text(&line(&a.content_id)));
    }
    f.child(Element::new("t:IsInline").text(if a.inline { "true" } else { "false" })).child(Element::new("t:Content").text(&base64(&a.data)))
}

/// What a draft is made of.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct DraftContent {
    /// The subject.
    pub subject: String,
    /// Whether the body is HTML, and the body.
    pub body: (bool, String),
    /// The recipients.
    pub to: Vec<Address>,
    /// See `to`.
    pub cc: Vec<Address>,
    /// See `to`.
    pub bcc: Vec<Address>,
    /// Where replies go (`Reply-To`).
    pub reply_to: Vec<Address>,
    /// The importance.
    pub importance: Importance,
    /// It asks for a read receipt (`IsReadReceiptRequested`).
    pub read_receipt: bool,
    /// The files.
    pub attachments: Vec<AttachmentUp>,
    /// MailCraft's marker, a value used once.
    pub marker: String,
}

impl std::fmt::Debug for DraftContent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DraftContent").field("attachments", &self.attachments.len()).finish()
    }
}

/// `CreateItem` with `SaveOnly`: a draft in the server's Drafts.
#[derive(Clone, PartialEq, Eq)]
pub struct CreateDraft {
    folder: Option<String>,
    content: DraftContent,
}

impl std::fmt::Debug for CreateDraft {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CreateDraft(..)")
    }
}

impl CreateDraft {
    /// Makes `content` in the folder `drafts` (its server id; none: the Drafts folder by its name).
    pub fn new(drafts: Option<&str>, content: DraftContent) -> Self {
        Self { folder: drafts.map(str::to_owned), content }
    }

    /// The operation: `SavedItemFolderId` and `Items`, with `MessageDisposition="SaveOnly"`.
    pub fn element(&self) -> Element {
        let c = &self.content;
        let mut message = Element::new("t:Message")
            .child(Element::new("t:Subject").text(&line(&c.subject)))
            .child(Element::new("t:Body").attr("BodyType", if c.body.0 { "HTML" } else { "Text" }).text(&c.body.1));
        if !c.attachments.is_empty() {
            message = message.child(c.attachments.iter().fold(Element::new("t:Attachments"), |list, a| list.child(file_attachment(a))));
        }
        let importance = match c.importance {
            Importance::Low => "Low",
            Importance::Normal => "Normal",
            Importance::High => "High",
        };
        message = message.child(Element::new("t:Importance").text(importance));
        if !c.marker.is_empty() {
            message = message.child(Element::new("t:ExtendedProperty").child(marker_uri()).child(Element::new("t:Value").text(&line(&c.marker))));
        }
        for (name, list) in [("t:ToRecipients", &c.to), ("t:CcRecipients", &c.cc), ("t:BccRecipients", &c.bcc)] {
            if list.iter().any(|a| !a.email.trim().is_empty()) {
                message = message.child(recipients(name, list));
            }
        }
        // (`IsReadReceiptRequested` comes after the recipients, `ReplyTo` after that.)
        if c.read_receipt {
            message = message.child(Element::new("t:IsReadReceiptRequested").text("true"));
        }
        if c.reply_to.iter().any(|a| !a.email.trim().is_empty()) {
            message = message.child(recipients("t:ReplyTo", &c.reply_to));
        }
        Element::new("m:CreateItem")
            .attr("MessageDisposition", "SaveOnly")
            .child(folder_ref("m:SavedItemFolderId", "drafts", self.folder.as_deref()))
            .child(Element::new("m:Items").child(message))
    }

    /// The draft's ids.
    pub fn answer(&self, reply: &Reply) -> Result<Saved, Error> {
        saved_of(reply, "CreateItem")
    }
}

/// `CreateItem` with `SaveOnly` and `MimeContent`: a draft kept Encrypted (#30 part 7) in the
/// server's Drafts, made from the Encrypted message itself, which is all that goes up of it. The
/// marker is put on it as on any draft, so that one whose make was not answered is found.
#[derive(Clone, PartialEq, Eq)]
pub struct CreateSealedDraft {
    mime: Vec<u8>,
    marker: String,
}

impl std::fmt::Debug for CreateSealedDraft {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CreateSealedDraft(..)")
    }
}

impl CreateSealedDraft {
    /// Makes the draft of `mime` (a whole message, which holds nothing in the clear but its header)
    /// in Drafts. Too large for any server: `None`.
    pub fn new(mime: &[u8], marker: &str) -> Option<Self> {
        (!mime.is_empty() && mime.len() <= MAX_MIME).then(|| Self { mime: mime.to_vec(), marker: marker.to_owned() })
    }

    /// The operation: `SavedItemFolderId` and `Items` with the MIME content and the marker.
    pub fn element(&self) -> Element {
        let mut message = Element::new("t:Message").child(Element::new("t:MimeContent").attr("CharacterSet", "UTF-8").text(&base64(&self.mime)));
        if !self.marker.is_empty() {
            message = message.child(Element::new("t:ExtendedProperty").child(marker_uri()).child(Element::new("t:Value").text(&line(&self.marker))));
        }
        Element::new("m:CreateItem")
            .attr("MessageDisposition", "SaveOnly")
            .child(folder_ref("m:SavedItemFolderId", "drafts", None))
            .child(Element::new("m:Items").child(message))
    }

    /// The draft's ids.
    pub fn answer(&self, reply: &Reply) -> Result<Saved, Error> {
        saved_of(reply, "CreateItem")
    }
}

fn saved_of(reply: &Reply, operation: &str) -> Result<Saved, Error> {
    let messages = reply.messages(operation)?;
    let [message] = messages.as_slice() else { return Err(Error::Unexpected) };
    let node = message.ok()?;
    let items = node.child_in(MESSAGES, "Items").ok_or(Error::Unexpected)?;
    let item = items.children.iter().find(|c| *c.namespace == *TYPES).ok_or(Error::Unexpected)?;
    let id = item.child_in(TYPES, "ItemId").ok_or(Error::Unexpected)?;
    let take = |name: &str| id.attribute(name).filter(|v| valid_id(v)).map(str::to_owned);
    Ok(Saved { id: take("Id").ok_or(Error::Unexpected)?, change_key: take("ChangeKey").unwrap_or_default() })
}

/// `FindItem` for the draft carrying a marker.
#[derive(Clone, PartialEq, Eq)]
pub struct FindDraft {
    folder: Option<String>,
    marker: String,
    /// The change key last known of the draft ("" when none was).
    known: String,
}

impl std::fmt::Debug for FindDraft {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FindDraft(..)")
    }
}

impl FindDraft {
    /// Looks in `drafts` (its server id; none: the Drafts folder by its name) for `marker`.
    pub fn new(drafts: Option<&str>, marker: &str) -> Self {
        Self { folder: drafts.map(str::to_owned), marker: marker.to_owned(), known: String::new() }
    }

    /// Of several drafts with the marker, the one with this change key is the one (else the newest).
    #[must_use]
    pub fn knowing(mut self, change_key: &str) -> Self {
        self.known = change_key.to_owned();
        self
    }

    /// The operation: `ItemShape`, `IndexedPageItemView`, `Restriction`, `ParentFolderIds`.
    pub fn element(&self) -> Element {
        let equal = Element::new("t:IsEqualTo")
            .child(marker_uri())
            .child(Element::new("t:FieldURIOrConstant").child(Element::new("t:Constant").attr("Value", &self.marker)));
        let parent = match self.folder.as_deref() {
            Some(id) if !id.is_empty() => id_element("t:FolderId", id, None),
            _ => Element::new("t:DistinguishedFolderId").attr("Id", "drafts"),
        };
        Element::new("m:FindItem")
            .attr("Traversal", "Shallow")
            .child(Element::new("m:ItemShape").child(Element::new("t:BaseShape").text("IdOnly")))
            .child(Element::new("m:IndexedPageItemView").attr("MaxEntriesReturned", "10").attr("Offset", "0").attr("BasePoint", "Beginning"))
            .child(Element::new("m:Restriction").child(equal))
            // (Newest first: the one to go on with when a make left two.)
            .child(Element::new("m:SortOrder").child(
                Element::new("t:FieldOrder").attr("Order", "Descending").child(Element::new("t:FieldURI").attr("FieldURI", "item:DateTimeCreated")),
            ))
            .child(Element::new("m:ParentFolderIds").child(parent))
    }

    /// The draft found: the only one; of several (a make whose answer was lost and then made
    /// again by an older program), the one whose change key was last known, else the newest. A
    /// third is never made.
    pub fn answer(&self, reply: &Reply) -> Result<Option<Saved>, Error> {
        let found = self.answer_all(reply)?;
        Ok(found.iter().find(|f| !self.known.is_empty() && f.change_key == self.known).or(found.first()).cloned())
    }

    /// Every draft found in the folder carrying the marker (at most the ten the view asks for): the
    /// earlier copies of one draft, which are all there is to take away (#30 part 7). They are in
    /// the server's Drafts, by what was asked, so each is a draft there.
    pub fn answer_all(&self, reply: &Reply) -> Result<Vec<Saved>, Error> {
        let messages = reply.messages("FindItem")?;
        let [message] = messages.as_slice() else { return Err(Error::Unexpected) };
        let node = message.ok()?;
        let root = node.child_in(MESSAGES, "RootFolder").ok_or(Error::Unexpected)?;
        let Some(items) = root.child_in(TYPES, "Items") else { return Ok(Vec::new()) };
        Ok(items
            .children
            .iter()
            .filter(|c| *c.namespace == *TYPES)
            .filter_map(|item| {
                let id = item.child_in(TYPES, "ItemId")?;
                let take = |name: &str| id.attribute(name).filter(|v| valid_id(v)).map(str::to_owned);
                Some(Saved { id: take("Id")?, change_key: take("ChangeKey").unwrap_or_default() })
            })
            .collect())
    }
}

/// `GetItem` for the copy of a draft that an Encrypted save takes the place of: its change key, whether
/// it is a draft, and the folder it is in.
#[derive(Clone, PartialEq, Eq)]
pub struct GetEarlier {
    id: String,
}

/// What [`GetEarlier`] reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EarlierItem {
    /// Its change key now.
    pub change_key: String,
    /// Whether it is a draft.
    pub is_draft: bool,
    /// The server's id of its folder.
    pub parent: String,
}

impl GetEarlier {
    /// Reads item `id`.
    pub fn new(id: &str) -> Self {
        Self { id: id.to_owned() }
    }

    /// The operation: the default shape holds the folder and `IsDraft`.
    pub fn element(&self) -> Element {
        let shape = Element::new("m:ItemShape").child(Element::new("t:BaseShape").text("Default"));
        Element::new("m:GetItem").child(shape).child(Element::new("m:ItemIds").child(id_element("t:ItemId", &self.id, None)))
    }

    /// The item now, or `ItemNotFound`.
    pub fn answer(&self, reply: &Reply) -> Result<EarlierItem, Error> {
        let messages = reply.messages("GetItem")?;
        let [message] = messages.as_slice() else { return Err(Error::Unexpected) };
        let node = message.ok()?;
        let items = node.child_in(MESSAGES, "Items").ok_or(Error::Unexpected)?;
        let item = items.children.iter().find(|c| *c.namespace == *TYPES).ok_or(Error::Unexpected)?;
        let take =
            |node: Option<&Node>, name: &str| node.and_then(|n| n.attribute(name)).filter(|v| valid_id(v)).map(str::to_owned).unwrap_or_default();
        Ok(EarlierItem {
            change_key: take(item.child_in(TYPES, "ItemId"), "ChangeKey"),
            is_draft: item.child_in(TYPES, "IsDraft").is_some_and(|n| n.text.trim().eq_ignore_ascii_case("true")),
            parent: take(item.child_in(TYPES, "ParentFolderId"), "Id"),
        })
    }
}

/// `CreateAttachment` on a draft.
#[derive(Clone, PartialEq, Eq)]
pub struct AddAttachments {
    parent: String,
    change_key: String,
    files: Vec<AttachmentUp>,
}

impl std::fmt::Debug for AddAttachments {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AddAttachments").field("files", &self.files.len()).finish()
    }
}

impl AddAttachments {
    /// Adds `files` to the draft `parent`.
    pub fn new(parent: &str, change_key: &str, files: Vec<AttachmentUp>) -> Self {
        Self { parent: parent.to_owned(), change_key: change_key.to_owned(), files }
    }

    /// The operation: `ParentItemId` and `Attachments`.
    pub fn element(&self) -> Element {
        Element::new("m:CreateAttachment")
            .child(id_element("m:ParentItemId", &self.parent, Some(&self.change_key)))
            .child(self.files.iter().fold(Element::new("m:Attachments"), |list, a| list.child(file_attachment(a))))
    }

    /// The attachments' ids in the order of the files, and the draft's change key now.
    pub fn answer(&self, reply: &Reply) -> Result<(Vec<String>, String), Error> {
        let messages = reply.messages("CreateAttachment")?;
        let mut ids = Vec::new();
        let mut key = String::new();
        for message in &messages {
            let node = message.ok()?;
            let list = node.child_in(MESSAGES, "Attachments").ok_or(Error::Unexpected)?;
            for a in list.children.iter().filter(|c| *c.namespace == *TYPES) {
                let id = a.child_in(TYPES, "AttachmentId").ok_or(Error::Unexpected)?;
                ids.push(id.attribute("Id").filter(|v| valid_id(v)).ok_or(Error::Unexpected)?.to_owned());
                if let Some(k) = id.attribute("RootItemChangeKey").filter(|v| valid_id(v)) {
                    key = k.to_owned();
                }
            }
        }
        if ids.len() != self.files.len() {
            return Err(Error::Unexpected);
        }
        Ok((ids, key))
    }
}

/// `DeleteAttachment` on a draft.
#[derive(Clone, PartialEq, Eq)]
pub struct RemoveAttachments {
    ids: Vec<String>,
}

impl std::fmt::Debug for RemoveAttachments {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RemoveAttachments(..)")
    }
}

impl RemoveAttachments {
    /// Takes the attachments `ids` off their draft.
    pub fn new(ids: Vec<String>) -> Self {
        Self { ids }
    }

    /// The operation: `AttachmentIds`.
    pub fn element(&self) -> Element {
        Element::new("m:DeleteAttachment")
            .child(self.ids.iter().fold(Element::new("m:AttachmentIds"), |list, id| list.child(id_element("t:AttachmentId", id, None))))
    }

    /// Whether they went, and the draft's change key now.
    pub fn answer(&self, reply: &Reply) -> Result<String, Error> {
        let messages = reply.messages("DeleteAttachment")?;
        let mut key = String::new();
        for message in &messages {
            let node = message.ok()?;
            if let Some(k) = node.child_in(MESSAGES, "RootItemId").and_then(|r| r.attribute("RootItemChangeKey")).filter(|v| valid_id(v)) {
                key = k.to_owned();
            }
        }
        if messages.len() != self.ids.len() {
            return Err(Error::Unexpected);
        }
        Ok(key)
    }
}

/// `CreateItem` with `SendAndSaveCopy`: a message never saved as a draft goes as its MIME, Bcc
/// included, and the server files the copy in Sent Items.
#[derive(Clone, PartialEq, Eq)]
pub struct SendMessage {
    mime: Vec<u8>,
    /// The server files the copy in Sent Items: the message itself. Not the message of a Bcc
    /// recipient (#30 part 7), which is only sent.
    save_copy: bool,
}

impl std::fmt::Debug for SendMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SendMessage(..)")
    }
}

impl SendMessage {
    /// Writes `message` as it goes. Not a message that can go (no recipient, an address that isn't
    /// one, too large) is said so, and nothing is sent.
    pub fn new(message: &Message) -> Result<Self, Unsendable> {
        if mailcraft_mime::signature_missing(message) {
            return Err(Unsendable::NotSigned);
        }
        if mailcraft_mime::encryption_missing(message) {
            return Err(Unsendable::NotEncrypted);
        }
        // (An Encrypted message whose Bcc recipients are each given a message of their own goes
        // to the others alone, see [`SendMessage::blind`].)
        Self::build(&message.for_main(), true)
    }

    /// The message each Bcc recipient of an Encrypted message is sent of their own (#30 part 7),
    /// to them alone and not filed in Sent Items: they go before the message itself.
    pub fn blind(message: &Message) -> Result<Vec<Self>, Unsendable> {
        message
            .blind_messages(true)
            .into_iter()
            .map(|(_, copy)| if mailcraft_mime::body_not_encrypted(&copy) { Err(Unsendable::NotEncrypted) } else { Self::build(&copy, false) })
            .collect()
    }

    /// Everything a message is sent as, in order, each with the Bcc recipient it is the message of
    /// (none for the message itself): theirs first, then the message everybody else gets, which is
    /// the one filed in Sent Items. A message to Bcc recipients alone has no message but theirs, and
    /// the last of theirs is the one filed. Nothing at all when every one of them has had theirs
    /// already and there is no more. Whatever can't go is found before any goes.
    pub fn plan(message: &Message) -> Result<Vec<(Option<String>, Self)>, Unsendable> {
        if mailcraft_mime::signature_missing(message) {
            return Err(Unsendable::NotSigned);
        }
        if mailcraft_mime::encryption_missing(message) {
            return Err(Unsendable::NotEncrypted);
        }
        let mut ops = Vec::new();
        for (address, copy) in message.blind_messages(true) {
            if mailcraft_mime::body_not_encrypted(&copy) {
                return Err(Unsendable::NotEncrypted);
            }
            ops.push((Some(address), Self::build(&copy, false)?));
        }
        if message.has_main_recipients() {
            ops.push((None, Self::build(&message.for_main(), true)?));
        } else if let Some((_, last)) = ops.last_mut() {
            last.save_copy = true;
        } else if message.blind_sent.is_empty() {
            return Err(Unsendable::NoRecipients);
        }
        Ok(ops)
    }

    fn build(message: &Message, save_copy: bool) -> Result<Self, Unsendable> {
        let mut anyone = false;
        for address in message.recipients() {
            let email = address.email.trim();
            if email.is_empty() {
                continue;
            }
            let ok = email.split_once('@').is_some_and(|(local, host)| !local.is_empty() && !host.is_empty())
                && !email.chars().any(|c| c.is_control() || c.is_whitespace() || c == '<' || c == '>');
            if !ok {
                return Err(Unsendable::BadAddress);
            }
            anyone = true;
        }
        if !anyone {
            return Err(Unsendable::NoRecipients);
        }
        let date = if message.date == 0 { mailcraft_time::now() } else { message.date };
        let dated = Message { date, ..message.clone() };
        let offset_minutes = mailcraft_time::local_offset_minutes(date);
        let mime = mailcraft_mime::write(&dated, &WriteOptions { include_bcc: true, offset_minutes });
        if mime.len() > MAX_MIME {
            return Err(Unsendable::TooLarge);
        }
        Ok(Self { mime, save_copy })
    }

    /// The operation: `SavedItemFolderId` (for the message itself) and `Items` with the MIME content.
    pub fn element(&self) -> Element {
        let message = Element::new("t:Message").child(Element::new("t:MimeContent").attr("CharacterSet", "UTF-8").text(&base64(&self.mime)));
        let items = Element::new("m:Items").child(message);
        if self.save_copy {
            Element::new("m:CreateItem")
                .attr("MessageDisposition", "SendAndSaveCopy")
                .child(folder_ref("m:SavedItemFolderId", "sentitems", None))
                .child(items)
        } else {
            Element::new("m:CreateItem").attr("MessageDisposition", "SendOnly").child(items)
        }
    }

    /// Whether the server took it (a send has no item to name: the copy is filed in Sent Items).
    pub fn answer(&self, reply: &Reply) -> Result<(), Error> {
        let messages = reply.messages("CreateItem")?;
        let [message] = messages.as_slice() else { return Err(Error::Unexpected) };
        message.ok().map(|_: &Node| ())
    }
}
