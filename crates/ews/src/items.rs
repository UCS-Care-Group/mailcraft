// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! `SyncFolderItems` and `GetItem` ([MS-OXWSSYNC], [MS-OXWSCORE], [MS-OXWSMSG]): a folder's
//! messages as headers and a preview first, and one message whole as its MIME content.
//!
//! **An item's id** is its `Id`, which stays the same while the item is where it is; the `ChangeKey`
//! changes with every edit, so it is kept apart (`ServerMessage::change_key`) and a message is
//! matched by its id alone. An item's server id changes when it is moved to another folder, so
//! a message moved on the server comes as a deletion and a new message.
//!
//! What a server doesn't say is left as it was (E7): an answer without a preview, a flag or the
//! categories reads as a message without them. Everything is capped as the answer is read.

use mailcraft_model::sync::{GraphMessage, ReadState, ServerMessage, ThreadOf};
use mailcraft_model::{Address, FlagStatus, Importance};

use crate::b64;
use crate::error::Error;
use crate::folder::valid_id;
use crate::reply::Reply;
use crate::soap::{Element, MESSAGES, TYPES};
use crate::xml::{MAX_MIME_TEXT, Node};

/// Most changes one page asks for (the schema's own limit is 512).
pub const MAX_CHANGES: u32 = 512;
/// The most changes one page holds that are taken in: what the model takes in of a page.
const MAX_PAGE: usize = mailcraft_model::sync::MAX_EWS_PAGE_MESSAGES;
/// Longest subject, `Message-ID`, preview and name kept, in characters; the most people a
/// message names in one list.
const MAX_SUBJECT: usize = 998;
const MAX_PREVIEW: usize = 256;
const MAX_NAME: usize = 256;
const MAX_ADDRESS: usize = 320;
const MAX_PEOPLE: usize = 200;
const MAX_CATEGORIES: usize = 50;
const MAX_CATEGORY: usize = 255;

/// What a list shows of an item that the `Default` shape doesn't give of every kind of item,
/// asked for by name in every shape (#1457). Of a meeting request, response or cancellation
/// (`MeetingRequest`, `MeetingResponse`, `MeetingCancellation`: [MS-OXWSMTGS]) Exchange Server
/// 2019 gives no time, size, sender, importance or `Message-ID` in that shape, and of a message
/// not the time it arrived or its importance; without them a meeting request was listed as sent
/// and received at no time, by nobody, at the bottom of the Inbox and the last to be fetched.
const LISTED: [&str; 6] = ["item:DateTimeReceived", "item:DateTimeSent", "item:Size", "item:Importance", "message:From", "message:InternetMessageId"];

/// Which shape of properties `SyncFolderItems` asks. Every one of them asks over the `IdOnly`
/// base and names everything it needs — since 2026-11-24 the shapes differ only in which
/// properties they can afford to name, on servers that refuse some (measured on the lab: the
/// `Default` base is what makes Kerio answer a listing with whole messages, see
/// [`SyncFolderItems::element`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    /// Everything a list shows: the recipients, the categories, the preview and the flag named
    /// with the rest. (Not the conversation, #795: a server that didn't know it would lose the
    /// preview and the flag with it; it is asked by itself, [`GetItemConversations`].)
    Full,
    /// Without the preview and the flag: for a server that doesn't know them (it answers
    /// `ErrorInvalidPropertyRequest`, or a schema fault), which are then left out.
    Lean,
    /// Without the recipients either: for a server that won't give them in `SyncFolderItems`
    /// (Exchange 2007 and 2010 answer `ErrorInvalidPropertyForOperation`, #1349). The
    /// recipients are then asked with `GetItem` ([`GetItemRecipients`]), where Microsoft
    /// documents them. (What this leaves to name is exactly what [`Shape::Ids`] asks without
    /// its flag — the same bytes on the wire, which is why a `Bare` run takes no ids fallback:
    /// it is already asking the sparsest ask there is.)
    Bare,
    /// The names of the run's schema shape, less the preview and the recipients — for the
    /// page that wants only what a change says: the bodies stay at home, opened when read.
    /// This is also the fallback for a server that answers a page heavy with named extras by
    /// hanging whole messages on every item of it (Kerio's servlet, #964 — which measures to
    /// answer this sparse naming with the properties alone). The read state is asked even
    /// here: an answer that omits it parses as unread, and an `Update` so parsed turns read
    /// messages unread on the way in (#1784).
    /// `flag` says whether the run's schema shape asks the flag: when it does (`Full`), an ids
    /// page asks `item:Flag` too, so a sparse `Update` under ids says what the flag is rather
    /// than nothing; a leaner run's server is one that doesn't know the flag, and asking it
    /// there would only be refused.
    Ids {
        /// The run's schema shape asked the flag, so this ids page asks it as well.
        flag: bool,
    },
}

/// The request `SyncFolderItems` of one folder.
#[derive(Clone, PartialEq, Eq)]
pub struct SyncFolderItems {
    folder: String,
    state: String,
    max: u32,
    shape: Shape,
    /// Ask the read state as an extended property too, as Outlook for Mac does (#1784).
    pub(crate) extended: bool,
}

impl std::fmt::Debug for SyncFolderItems {
    /// Nothing the server said: counts and flags.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SyncFolderItems").field("max", &self.max).field("shape", &self.shape).field("state", &!self.state.is_empty()).finish()
    }
}

/// One page of a folder's changes.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct ItemsPage {
    /// The state to go on from.
    pub state: String,
    /// The page reaches the last change the server has (`IncludesLastItemInRange`).
    pub last: bool,
    /// Messages created or updated, whole as a list shows them (the folder is not said: the caller
    /// knows it).
    pub messages: Vec<GraphMessage>,
    /// The ids of the items deleted.
    pub removed: Vec<String>,
    /// Items whose read state alone changed.
    pub read: Vec<ReadState>,
    /// How many `ReadFlagChange`s said no read state and were passed over (they carry nothing to lose).
    pub read_unsaid: usize,
}

impl std::fmt::Debug for ItemsPage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ItemsPage")
            .field("last", &self.last)
            .field("messages", &self.messages.len())
            .field("removed", &self.removed.len())
            .field("read", &self.read.len())
            .finish()
    }
}

impl SyncFolderItems {
    /// Reads folder `folder` (its server id) from `state` (empty: from its start), `max` changes
    /// at the most (1 to 512).
    pub fn new(folder: &str, state: &str, max: u32, shape: Shape) -> Self {
        Self { folder: folder.to_owned(), state: state.to_owned(), max: max.clamp(1, MAX_CHANGES), shape, extended: false }
    }

    /// The operation as the schema has it: `ItemShape` (the `IdOnly` base with every property a
    /// list shows it may lack named — [`LISTED`] and the shape's own; the `Default` base is no
    /// longer asked of anyone), `SyncFolderId`, `SyncState`, `MaxChangesReturned`, `SyncScope`.
    ///
    /// (The `Default` base was measured into retirement on the lab server, 2026-11-24: six
    /// messages of 215 MB between them answer as 7 KB over `IdOnly` with everything named —
    /// which is Outlook for Mac's own ask, captured through a proxy — and as 287 MB, whole
    /// MIME bodies and all, over `Default` with four properties named on top. Kerio's servlet
    /// hangs the message on every item of a `Default` page; naming the properties is what
    /// keeps the answer to the properties. Nothing the parser reads was ever a reason to ask
    /// `Default`: every property a list shows is named below, and the id and change key the
    /// base also brings come over `IdOnly` like everything else.)
    pub fn element(&self) -> Element {
        let uri = |name: &str| Element::new("t:FieldURI").attr("FieldURI", name);
        let mut extra = LISTED.iter().fold(Element::new("t:AdditionalProperties"), |all, name| all.child(uri(name)));
        // (What the `Default` base used to bring by itself, now named for every shape: the
        // subject, the attachment flag and the read and draft state. The read state above
        // all: an answer without it parses as unread, and an `Update` so parsed turns read
        // messages unread on the way in (#1784, benandez's repro). Kerio 10.0.9 answers
        // `message:IsRead` both ways — measured 2026-11-24 across read and unread messages
        // in one mailbox; older builds answer it silent, which is what the extended tag
        // below is still there for. `item:IsDraft`, not `message:` it: the schema puts the
        // flag on the item, and Kerio answers a bare SOAP fault for the misnamed one,
        // measured 2026-11-24 — which is likely some of the old "refuses the ids asking"
        // behaviour, the asking earning a refusal its name had not earned.)
        extra = extra.child(uri("item:Subject")).child(uri("item:HasAttachments")).child(uri("message:IsRead")).child(uri("item:IsDraft"));
        if self.shape == Shape::Full || self.shape == Shape::Lean {
            extra = extra.child(uri("message:ToRecipients")).child(uri("message:CcRecipients"));
        }
        extra = extra.child(uri("item:Categories"));
        if self.shape == Shape::Full {
            extra = extra.child(uri("item:Preview")).child(uri("item:Flag"));
        }
        // (`Ids` asks the flag only when its run's schema shape does: a server lean enough to
        // have lost the flag refuses it, and the refusal would lose the page.)
        if let Shape::Ids { flag: true } = self.shape {
            extra = extra.child(uri("item:Flag"));
        }
        // (The read state as MAPI knows it, `PR_IS_READ` (0x0E67), next to the SOAP one: older
        // Kerio answers the listing silently over `IsRead` yet answers this tag, which is how
        // Outlook for Mac knows a message is read on it (#964, #1784). It rides every page of
        // the run, the schema shapes with the sparse one: measured 2026-11-24, the tag answers
        // lean inside an `IdOnly` page over 215 MB of mail — the old crash was the weight of
        // what the page asked for, not the door.
        // The schema's own spelling, leading `0x` and all (as the Outlook capture spells every
        // tag): the bare `0E67` Kerio answers by dropping the connection, measured twice
        // 2026-11-24, servlet and all.)
        // (The flag as MAPI knows it, `PR_FLAG_STATUS` (0x1090), next to the read tag: Kerio's
        // schema has no `item:Flag` at all — naming it poisons the whole page, the same fault
        // class as a misspelling, measured 2026-11-24 — and the flag its web mail sets and its
        // IMAP clients set as `\Flagged` lives behind this tag, which answers lean inside an
        // `IdOnly` page and maps both ways (measured the same day). Outlook asks this tag too.)
        if self.extended {
            extra = extra
                .child(Element::new("t:ExtendedFieldURI").attr("PropertyTag", "0x0E67").attr("PropertyType", "Boolean"))
                .child(Element::new("t:ExtendedFieldURI").attr("PropertyTag", "0x1090").attr("PropertyType", "Integer"))
                .child(Element::new("t:ExtendedFieldURI").attr("PropertyTag", "0x1081").attr("PropertyType", "Integer"));
        }
        let shape = Element::new("m:ItemShape").child(Element::new("t:BaseShape").text("IdOnly")).child(extra);
        let folder = Element::new("m:SyncFolderId").child(Element::new("t:FolderId").attr("Id", &self.folder));
        let mut operation = Element::new("m:SyncFolderItems").child(shape).child(folder);
        if !self.state.is_empty() {
            operation = operation.child(Element::new("m:SyncState").text(&self.state));
        }
        operation.child(Element::new("m:MaxChangesReturned").text(&self.max.to_string())).child(Element::new("m:SyncScope").text("NormalItems"))
    }

    /// The server's answer. A state the server no longer knows is [`Error::Server`] of
    /// `InvalidSyncState`.
    ///
    /// **A page is whole or it is an error.** The answer's new state says "everything up to here
    /// has been taken in", so a change that was left out would be lost for good. Every change is
    /// taken in, however many the server gave (the size of the answer and its nodes are held
    /// already, as the answer is parsed, and a server that ignores `MaxChangesReturned` has said its
    /// state covers them all); one that can't be taken in makes the page [`Error::Unreadable`], with
    /// nothing of it handed on and no state taken from it, so the folder stays where it was and is
    /// read again next time. Only a change with no usable id (a `Delete`, a `ReadFlagChange`, an
    /// item) or no item (a `Create` or an `Update` with nothing in it) can't be taken in: whatever
    /// else an item lacks (its read state, importance, categories, preview, times) is read as
    /// nothing said, as a message without it. A `ReadFlagChange` that doesn't say which state it
    /// is carries nothing to lose, and is passed over. A kind of change this program doesn't know
    /// is passed over too (E7).
    pub fn answer(&self, reply: &Reply) -> Result<ItemsPage, Error> {
        let messages = reply.messages("SyncFolderItems")?;
        let [message] = messages.as_slice() else { return Err(Error::Unexpected) };
        let node = message.ok()?;
        let state = node.child_in(MESSAGES, "SyncState").map(|n| n.text.trim().to_owned()).filter(|s| !s.is_empty()).ok_or(Error::Unexpected)?;
        let last = node.child_in(MESSAGES, "IncludesLastItemInRange").is_none_or(|n| n.text.trim() != "false");
        let mut page = ItemsPage { state, last, ..ItemsPage::default() };
        let Some(changes) = node.child_in(MESSAGES, "Changes") else { return Ok(page) };
        // (What the model takes in of one page: more than that is a page that can't be taken in.)
        if changes.children.len() > MAX_PAGE {
            return Err(Error::Unreadable);
        }
        for change in changes.children.iter().filter(|c| *c.namespace == *TYPES) {
            let id = || change.child_in(TYPES, "ItemId").and_then(|id| id.attribute("Id")).filter(|id| valid_id(id));
            match change.name.as_str() {
                "Create" | "Update" => {
                    let item = change.children.iter().find(|c| *c.namespace == *TYPES).and_then(read_item);
                    page.messages.push(item.ok_or(Error::Unreadable)?);
                }
                "Delete" => page.removed.push(id().ok_or(Error::Unreadable)?.to_owned()),
                "ReadFlagChange" => {
                    let id = id().ok_or(Error::Unreadable)?;
                    match change.child_in(TYPES, "IsRead").map(|n| n.text.trim().eq_ignore_ascii_case("true")) {
                        Some(read) => page.read.push(ReadState { id: id.to_owned(), read }),
                        None => page.read_unsaid = page.read_unsaid.saturating_add(1),
                    }
                }
                _ => {}
            }
        }
        Ok(page)
    }
}

/// `text` on one line, `most` characters at the most.
fn line(text: &str, most: usize) -> String {
    let mut out = String::new();
    for word in text.split(|c: char| c.is_whitespace() || c.is_control()).filter(|w| !w.is_empty()) {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
        if out.chars().count() >= most {
            break;
        }
    }
    out.chars().take(most).collect()
}

/// The time an element says (`2013-08-29T15:22:10Z`); 0 when it says none that can be read.
fn time(node: &Node, name: &str) -> i64 {
    node.child_in(TYPES, name).and_then(|n| mailcraft_time::iso_to_timestamp(n.text.trim(), 0)).unwrap_or(0)
}

/// An item's answer to one extended-property tag: the `Value` text, when the answer names it.
/// Servers quote the tag every way the schema allows — hex with or without the `0x` and in
/// either case, and plain decimal — so all do. The decimal is written out because a
/// prefix-less hex spelling reads back as whatever base it matches, so the two spellings of
/// one tag must never collide with another tag's two spellings; a tag added here must keep
/// that true (0x0E67/3687, 0x1081/4225, 0x1090/4240 are clean apart).
fn extended_value<'a>(item: &'a Node, decimal: &str, hex: u32) -> Option<&'a str> {
    item.children
        .iter()
        .filter(|p| p.is(TYPES, "ExtendedProperty"))
        .find(|p| {
            p.child_in(TYPES, "ExtendedFieldURI").and_then(|u| u.attribute("PropertyTag")).is_some_and(|tag| {
                let tag = tag.trim();
                tag == decimal || u32::from_str_radix(tag.trim_start_matches("0x").trim_start_matches("0X"), 16) == Ok(hex)
            })
        })
        .and_then(|p| p.child_in(TYPES, "Value"))
        .map(|v| v.text.trim())
}

/// The item's read state as an extended property: `PR_IS_READ` (0x0E67). None: the answer
/// didn't name it, or didn't answer it in words; a value that means neither yes nor no is no
/// answer (#1784).
fn extended_read(item: &Node) -> Option<bool> {
    extended_value(item, "3687", 0x0E67).and_then(|t| match t {
        "1" => Some(true),
        "0" => Some(false),
        t if t.eq_ignore_ascii_case("true") => Some(true),
        t if t.eq_ignore_ascii_case("false") => Some(false),
        _ => None,
    })
}

/// The answered and forwarded marks as `PR_ICON_ID` (0x1081): the icon a MAPI client draws,
/// which numbers what the message has had done to it — 102 is a reply sent, 104 a forward
/// ([MS-OXOCORG]'s icon table, the icons Outlook itself picks). A server that computes the
/// icon from its real marks answers it lean: measured on Kerio 2026-11-24, a web-mail or
/// IMAP `\Answered` came back 102 and `\Forwarded` 104, which is how Outlook shows a replied
/// arrow there — the same door, read. One icon says one thing: a message both answered and
/// forwarded shows the one the server's table ranks first, and the other mark waits for a
/// server that names them apart.
fn extended_marks(item: &Node) -> (bool, bool) {
    match extended_value(item, "4225", 0x1081).and_then(|v| v.parse::<i64>().ok()) {
        Some(102) => (true, false),
        Some(104) => (false, true),
        Some(_) => (false, false),
        None => (false, false),
    }
}

fn flag(node: &Node, name: &str) -> bool {
    node.child_in(TYPES, name).is_some_and(|n| n.text.trim().eq_ignore_ascii_case("true"))
}

/// The item's flag as an extended property: `PR_FLAG_STATUS` (0x1090), the flag as MAPI
/// numbers it ([MS-OXOFLAG], same numbering as Outlook's `OlFlagStatus`): 0 is no flag,
/// 1 is a flag worked through, 2 is flagged; anything else is no answer. The tag is
/// quoted as the server likes, read the way [`extended_read`] reads its own — decimal
/// only ever matches 4240 and a prefix-less hex spelling only ever reads back as this
/// tag, the two not colliding; a tag added this way must keep that true. On Kerio this
/// is the only flag there is (#1784, the flag: measured 2026-11-24, the tag answers lean
/// and follows what the web mail and IMAP's `\Flagged` set — its own web mail's flag
/// coming back as 2, a completed follow-up as 1).
fn extended_flag(item: &Node) -> Option<FlagStatus> {
    extended_value(item, "4240", 0x1090).and_then(|t| t.parse::<i64>().ok()).and_then(|value| match value {
        0 => Some(FlagStatus::None),
        1 => Some(FlagStatus::Complete),
        2 => Some(FlagStatus::Flagged),
        _ => None,
    })
}

/// A `Mailbox` element as an address. An address in the server's own directory (routing type
/// `EX`) is not an address anybody can write to: only the name is kept.
fn mailbox(node: &Node) -> Option<Address> {
    let name = line(node.child_in(TYPES, "Name").map_or("", |n| n.text.as_str()), MAX_NAME);
    let routing = node.child_in(TYPES, "RoutingType").map_or("SMTP", |n| n.text.trim());
    let email = if routing.eq_ignore_ascii_case("SMTP") {
        line(node.child_in(TYPES, "EmailAddress").map_or("", |n| n.text.as_str()), MAX_ADDRESS)
    } else {
        String::new()
    };
    (!name.is_empty() || !email.is_empty()).then_some(Address { name, email })
}

/// The people in a list element (`ToRecipients`), or the one in `From`.
fn people(node: &Node, name: &str) -> Vec<Address> {
    let Some(list) = node.child_in(TYPES, name) else { return Vec::new() };
    let one = |holder: &Node| if holder.is(TYPES, "Mailbox") { mailbox(holder) } else { holder.child_in(TYPES, "Mailbox").and_then(mailbox) };
    list.children.iter().take(MAX_PEOPLE).filter_map(one).collect()
}

/// The conversation an item is in (`ConversationId`, an id as an item's is: #795), else none.
pub(crate) fn conversation_of(item: &Node) -> String {
    let id = item.child_in(TYPES, "ConversationId").and_then(|n| n.attribute("Id")).filter(|id| valid_id(id)).unwrap_or_default();
    mailcraft_model::sync::thread_id(id)
}

/// The message in an item element, when it has an id.
fn read_item(item: &Node) -> Option<GraphMessage> {
    let id_node = item.child_in(TYPES, "ItemId")?;
    let id = id_node.attribute("Id").filter(|id| valid_id(id))?;
    let change_key = id_node.attribute("ChangeKey").filter(|key| valid_id(key)).unwrap_or_default();
    let importance = match item.child_in(TYPES, "Importance").map_or("", |n| n.text.trim()) {
        "Low" => Importance::Low,
        "High" => Importance::High,
        _ => Importance::Normal,
    };
    let flag_node = item.child_in(TYPES, "Flag");
    let ext_flag = extended_flag(item);
    // The schema flag answers with the MAPI tag beside it: the tag is a fallback for servers
    // whose schema has no flag family at all, never an override — a `NotFlagged` answered by
    // the node outvotes any tag value (#1784). Dates parse from the node alone: the tag says
    // nothing of them, and the servers that answer the tag without the node have no dates
    // to say (Kerio measured 2026-11-24), so `flag_said` covers one flag whole either way.
    let status = match flag_node {
        Some(f) => match f.child_in(TYPES, "FlagStatus").map_or("", |n| n.text.trim()) {
            "Flagged" => FlagStatus::Flagged,
            "Complete" => FlagStatus::Complete,
            _ => FlagStatus::None,
        },
        None => ext_flag.unwrap_or(FlagStatus::None),
    };
    let when = |name: &str| flag_node.and_then(|f| f.child_in(TYPES, name)).and_then(|n| mailcraft_time::iso_to_timestamp(n.text.trim(), 0));
    let cats_node = item.child_in(TYPES, "Categories");
    let categories = cats_node
        .map(|c| c.children.iter().take(MAX_CATEGORIES).map(|s| line(&s.text, MAX_CATEGORY)).filter(|s| !s.is_empty()).collect())
        .unwrap_or_default();
    let message_id = line(item.child_in(TYPES, "InternetMessageId").map_or("", |n| n.text.as_str()), MAX_SUBJECT);
    let native_read = item.child_in(TYPES, "IsRead").map(|n| n.text.trim().eq_ignore_ascii_case("true"));
    let ext_read = extended_read(item);
    let (answered, forwarded) = extended_marks(item);
    Some(GraphMessage {
        server: ServerMessage {
            id: id.to_owned(),
            read: native_read.or(ext_read).unwrap_or(false),
            read_said: native_read.is_some() || ext_read.is_some(),
            answered,
            forwarded,
            flag: status,
            flag_said: flag_node.is_some() || ext_flag.is_some(),
            categories,
            categories_said: cats_node.is_some(),
            importance,
            importance_said: item.child_in(TYPES, "Importance").is_some(),
            size: item.child_in(TYPES, "Size").and_then(|n| n.text.trim().parse().ok()).unwrap_or(0),
            flag_start: when("StartDate").filter(|_| status != FlagStatus::None),
            flag_due: when("DueDate").filter(|_| status != FlagStatus::None),
            flag_completed: when("CompleteDate").filter(|_| status == FlagStatus::Complete),
            change_key: change_key.to_owned(),
            thread: conversation_of(item),
            ..ServerMessage::default()
        },
        message_id: message_id.trim_start_matches('<').trim_end_matches('>').to_owned(),
        subject: line(item.child_in(TYPES, "Subject").map_or("", |n| n.text.as_str()), MAX_SUBJECT),
        from: people(item, "From").into_iter().next().unwrap_or_default(),
        to: people(item, "ToRecipients"),
        cc: people(item, "CcRecipients"),
        sent: time(item, "DateTimeSent"),
        received: time(item, "DateTimeReceived"),
        preview: line(item.child_in(TYPES, "Preview").map_or("", |n| n.text.as_str()), MAX_PREVIEW),
        has_attachments: flag(item, "HasAttachments"),
        draft: item.child_in(TYPES, "IsDraft").map(|n| n.text.trim().eq_ignore_ascii_case("true")),
    })
}

/// Most items one `GetItem` asks the recipients of.
pub const MAX_RECIPIENTS_ASKED: usize = 100;

/// The people a message was sent to, as [`GetItemRecipients`] reads them.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Recipients {
    /// The item's id.
    pub id: String,
    /// `ToRecipients`.
    pub to: Vec<Address>,
    /// `CcRecipients`.
    pub cc: Vec<Address>,
}

impl std::fmt::Debug for Recipients {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Recipients").field("to", &self.to.len()).field("cc", &self.cc.len()).finish()
    }
}

/// The request `GetItem` for the recipients of several items: for a server whose `SyncFolderItems`
/// won't give them ([`Shape::Bare`], #1349), or leaves them out (Exchange 2013 to 2019, #1151).
#[derive(Clone, PartialEq, Eq)]
pub struct GetItemRecipients {
    ids: Vec<String>,
}

impl std::fmt::Debug for GetItemRecipients {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GetItemRecipients").field("ids", &self.ids.len()).finish()
    }
}

impl GetItemRecipients {
    /// Asks for the items with these ids, the first [`MAX_RECIPIENTS_ASKED`] of them that are ids.
    pub fn new(ids: &[String]) -> Self {
        Self { ids: ids.iter().filter(|id| valid_id(id)).take(MAX_RECIPIENTS_ASKED).cloned().collect() }
    }

    /// Whether there is nothing to ask.
    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    /// The operation as the schema has it: `ItemShape` (`IdOnly` and the two lists), `ItemIds`.
    pub fn element(&self) -> Element {
        let uri = |name: &str| Element::new("t:FieldURI").attr("FieldURI", name);
        let extra = Element::new("t:AdditionalProperties").child(uri("message:ToRecipients")).child(uri("message:CcRecipients"));
        let shape = Element::new("m:ItemShape").child(Element::new("t:BaseShape").text("IdOnly")).child(extra);
        let ids = self.ids.iter().fold(Element::new("m:ItemIds"), |all, id| all.child(Element::new("t:ItemId").attr("Id", id)));
        Element::new("m:GetItem").child(shape).child(ids)
    }

    /// The recipients of each item the server answered for, in the order asked; an item it answered
    /// with an error of its own (gone since, say) is left out. A busy server is [`Error::Busy`] for
    /// the lot.
    pub fn answer(&self, reply: &Reply) -> Result<Vec<Recipients>, Error> {
        let messages = reply.messages("GetItem")?;
        let mut out = Vec::with_capacity(self.ids.len());
        for (id, message) in self.ids.iter().zip(&messages) {
            match message.ok() {
                Ok(node) => {
                    let Some(item) = node.child_in(MESSAGES, "Items").and_then(|items| items.children.iter().find(|c| *c.namespace == *TYPES)) else {
                        continue;
                    };
                    out.push(Recipients { id: id.clone(), to: people(item, "ToRecipients"), cc: people(item, "CcRecipients") });
                }
                Err(busy @ Error::Busy { .. }) => return Err(busy),
                Err(_) => {}
            }
        }
        Ok(out)
    }
}

/// Most items one `GetItem` asks the conversation of.
pub const MAX_CONVERSATIONS: usize = 100;

/// The request `GetItem` for the conversations of several items (`ConversationId`, #795): asked
/// of mail once it is listed, since the folders' changes don't ask for it.
#[derive(Clone, PartialEq, Eq)]
pub struct GetItemConversations {
    ids: Vec<String>,
}

impl std::fmt::Debug for GetItemConversations {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GetItemConversations").field("ids", &self.ids.len()).finish()
    }
}

impl GetItemConversations {
    /// Asks for the items with these ids (server ids from a sync), the first
    /// [`MAX_CONVERSATIONS`] of them that are ids.
    pub fn new(ids: &[String]) -> Self {
        Self { ids: ids.iter().filter(|id| valid_id(id)).take(MAX_CONVERSATIONS).cloned().collect() }
    }

    /// The items asked about, in order.
    pub fn ids(&self) -> &[String] {
        &self.ids
    }

    /// The operation as the schema has it: `ItemShape` (`IdOnly` and the `ConversationId`), `ItemIds`.
    pub fn element(&self) -> Element {
        let extra = Element::new("t:AdditionalProperties").child(Element::new("t:FieldURI").attr("FieldURI", "item:ConversationId"));
        let shape = Element::new("m:ItemShape").child(Element::new("t:BaseShape").text("IdOnly")).child(extra);
        let ids = self.ids.iter().fold(Element::new("m:ItemIds"), |all, id| all.child(Element::new("t:ItemId").attr("Id", id)));
        Element::new("m:GetItem").child(shape).child(ids)
    }

    /// What the server said of each item, in the order asked: its conversation (empty for none, or
    /// for an item the server no longer has), or refused when the server answered for it with an
    /// error of its own; an item it gave no answer for isn't there. A busy server is
    /// [`Error::Busy`] for the lot.
    pub fn answer(&self, reply: &Reply) -> Result<Vec<ThreadOf>, Error> {
        let messages = reply.messages("GetItem")?;
        let mut out = Vec::with_capacity(self.ids.len());
        for (id, message) in self.ids.iter().zip(&messages) {
            out.push(match message.ok() {
                Ok(node) => {
                    let item = node.child_in(MESSAGES, "Items").and_then(|items| items.children.iter().find(|c| *c.namespace == *TYPES));
                    ThreadOf { id: id.clone(), thread: item.map(conversation_of).unwrap_or_default(), refused: false }
                }
                Err(Error::Server(crate::error::ResponseCode::ItemNotFound)) => ThreadOf { id: id.clone(), ..ThreadOf::default() },
                Err(busy @ Error::Busy { .. }) => return Err(busy),
                Err(Error::Server(_)) => ThreadOf::refused(id),
                Err(_) => continue,
            });
        }
        Ok(out)
    }
}

/// The request `GetItem` for one message's MIME content.
#[derive(Clone, PartialEq, Eq)]
pub struct GetItemMime {
    id: String,
}

impl std::fmt::Debug for GetItemMime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("GetItemMime(..)")
    }
}

impl GetItemMime {
    /// Asks for the item with this id (a server id from a sync).
    pub fn new(id: &str) -> Self {
        Self { id: id.to_owned() }
    }

    /// The operation as the schema has it: `ItemShape` (`IdOnly` and `IncludeMimeContent`), `ItemIds`.
    pub fn element(&self) -> Element {
        let shape =
            Element::new("m:ItemShape").child(Element::new("t:BaseShape").text("IdOnly")).child(Element::new("t:IncludeMimeContent").text("true"));
        let ids = Element::new("m:ItemIds").child(Element::new("t:ItemId").attr("Id", &self.id));
        Element::new("m:GetItem").child(shape).child(ids)
    }

    /// The message as the server holds it (its MIME), or why not: [`Error::Server`] of
    /// `ItemNotFound` for an item it no longer has. A message the answer holds more of than
    /// can be read (over [`MAX_MIME_TEXT`] of base64) is [`Error::TooLarge`].
    pub fn answer(&self, reply: &Reply) -> Result<Vec<u8>, Error> {
        let messages = reply.messages("GetItem")?;
        let [message] = messages.as_slice() else { return Err(Error::Unexpected) };
        let node = message.ok()?;
        let items = node.child_in(MESSAGES, "Items").ok_or(Error::Unexpected)?;
        let item = items.children.iter().find(|c| *c.namespace == *TYPES).ok_or(Error::Unexpected)?;
        let content = item.child_in(TYPES, "MimeContent").ok_or(Error::Unexpected)?;
        if content.text.is_empty() {
            // (Either nothing, or more than the reader keeps.)
            return Err(Error::TooLarge);
        }
        let bytes = b64::decode(&content.text, MAX_MIME_TEXT).ok_or(Error::Unexpected)?;
        if bytes.is_empty() { Err(Error::Unexpected) } else { Ok(bytes) }
    }
}
