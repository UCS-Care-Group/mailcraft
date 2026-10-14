//! The Favorites of the folder pane as classic Outlook keeps them, read through Exchange Web
//! Services (EWS) and never written: for an account of an organisation whose owner chose this
//! way of reading them rather than Microsoft Graph's ([`crate::graph::favorites`]). What comes
//! back is the same download, so everything after this module is shared.
//!
//! **It exists until 1 April 2027**, when Microsoft switches EWS off in Exchange Online for
//! everybody; an organisation may have switched it off already, or keep it for the programs on
//! a list of its own. Nothing but the Favorites is read through it, and nothing here can write:
//! the only operations are `FindFolder`, `FindItem` and `GetFolder`.
//!
//! Classic Outlook keeps the Favorites as navigation shortcuts: hidden ("associated") messages
//! of class `IPM.Microsoft.WunderBar.Link` in the mailbox's Common Views folder ([MS-OXOCFG]
//! 2.2.9 and 3.1.4.9; [`links`] says which of their properties are read and how they are put
//! in order). Reading them takes four questions, all `POST`ed as SOAP 1.1 to
//! `https://outlook.office365.com/EWS/Exchange.asmx` with the access token of the scope
//! `https://outlook.office365.com/EWS.AccessAsUser.All` ([`crate::oauth::refresh_ews`]):
//!
//! 1. `FindFolder`, `Traversal="Shallow"`, under the distinguished folder `root`, for the
//!    folder whose `folder:DisplayName` is `Common Views`. (EWS has no name of its own for that
//!    folder; [MS-OXOSFLD] lists it under the root of every mailbox.) A mailbox that has none
//!    has no list: [`FavoritesOutcome::Missing`].
//! 2. `FindItem`, `Traversal="Associated"`, in that folder, for the items whose
//!    `item:ItemClass` is `IPM.Microsoft.WunderBar.Link`, with the six properties [`links`]
//!    names as `ExtendedFieldURI`s. In pages (`IndexedPageItemView`), to the end.
//! 3. `GetFolder` for the distinguished folders `msgfolderroot` (where a folder's path ends),
//!    `inbox`, `drafts`, `sentitems`, `deleteditems`, `junkemail` and `outbox`: their ids, so
//!    that an Inbox that is called something else is still known for the Inbox. (EWS has no
//!    distinguished id for the Archive folder of a mailbox, only for an archive mailbox: an
//!    Archive folder is found by its name, like any other.)
//! 4. `FindFolder`, `Traversal="Deep"`, under `msgfolderroot`: every folder with its
//!    `folder:DisplayName`, its `folder:ParentFolderId` and its EntryID (the property 0x0FFF,
//!    `Binary`), in pages (`IndexedPageFolderView`), to the end. A shortcut names its folder by
//!    EntryID ([MS-OXOCFG] 2.2.9.8), which is how the two are matched ([`links::key_of`]); the
//!    names from the top of the mailbox down are how the mailbox here finds its own folder.
//!    No id of the server's leaves this module.
//!
//! The elements, their order and their attributes are the ones of the public EWS reference
//! (`FindFolder`, `FindItem`, `GetFolder`, `ExtendedFieldURI`, `DistinguishedFolderId`,
//! `IndexedPageItemView`) and of the schemas in [MS-OXWSSRCH], [MS-OXWSFOLD], [MS-OXWSCDATA]
//! and [MS-OXWSXPROP]; the service picks the operation from the body's own element, so no
//! `SOAPAction` goes along. **None of it has met the real service**: it is written from the
//! documents alone.
//!
//! What can go wrong, for the host to act on:
//! - [`NetError::Blocked`]: the organisation doesn't allow EWS, for anybody or for this program
//!   (HTTP 403 with an `X-EWS-Policy-Reason` header; the header is what says so, never the
//!   body). Asking again changes nothing; Microsoft Graph is the other way.
//! - `ProtoError::Auth`: the service didn't take the token (401). Get another.
//! - [`NetError::Forbidden`]: the token is good and the account may not do this (a 403 without
//!   that header, `ErrorAccessDenied`).
//! - [`NetError::Busy`]: not now (429, 503, 504, `ErrorServerBusy`).
//! - anything else: the service couldn't be reached, or answered something that isn't the
//!   answer (a SOAP fault, a page that is cut short, XML that can't be read): nothing is
//!   concluded from it, and a list is never passed off as whole that isn't.
//!
//! The answers are hostile like any server's ([`xml`]). The log says how many shortcuts there
//! were and what became of them, never a name or an id; the token appears nowhere.

mod links;
mod xml;

#[cfg(any(test, feature = "stand-in"))]
pub mod standin;

use std::collections::HashMap;
use std::time::{Duration, Instant};

use mailcraft_model::FolderKind;
use mailcraft_model::remote::{FavoritesDownload, FavoritesOutcome, MAX_FAVORITES};
use mailcraft_proto::ProtoError;

use self::links::Link;
use self::xml::Node;
use crate::NetError;
use crate::http::{self, Reply, Request, Url};

/// Where Exchange Online answers Exchange Web Services.
const ENDPOINT: &str = "https://outlook.office365.com/EWS/Exchange.asmx";
/// The path of the service, on that host or on a stand-in's.
#[cfg(any(test, feature = "stand-in"))]
const PATH: &str = "/EWS/Exchange.asmx";
/// The namespaces of a request, and the version of the schema it is written to (the newest the
/// reference of `RequestServerVersion` names; everything asked for here is far older).
const SOAP: &str = "http://schemas.xmlsoap.org/soap/envelope/";
const TYPES: &str = "http://schemas.microsoft.com/exchange/services/2006/types";
const MESSAGES: &str = "http://schemas.microsoft.com/exchange/services/2006/messages";
const VERSION: &str = "Exchange2013_SP1";
/// The folder the shortcuts are kept in, and the class of a shortcut.
const COMMON_VIEWS: &str = "Common Views";
const LINK_CLASS: &str = "IPM.Microsoft.WunderBar.Link";
/// The special folders EWS has distinguished ids for, and the folder a path ends at.
const DISTINGUISHED: [(&str, FolderKind); 6] = [
    ("inbox", FolderKind::Inbox),
    ("drafts", FolderKind::Drafts),
    ("sentitems", FolderKind::Sent),
    ("deleteditems", FolderKind::Deleted),
    ("junkemail", FolderKind::Junk),
    ("outbox", FolderKind::Outbox),
];
const TOP: &str = "msgfolderroot";
/// The EntryID of a folder, as an extended property (`PidTagEntryId`).
const ENTRY_ID: u16 = 0x0FFF;
/// Shortcuts and folders asked for per page (the service gives no more than 250 of what a
/// restriction picks, and 1,000 otherwise), and the most pages of each that are read.
const LINK_PAGE: usize = 200;
const MAX_LINK_PAGES: usize = 50;
const TREE_PAGE: usize = 500;
const MAX_TREE_PAGES: usize = 40;
/// The largest answer that is read, the longest id of the service's that is taken (and sent
/// back to it), and the longest name of a folder.
const MAX_ANSWER: usize = 8 * 1024 * 1024;
const MAX_ID: usize = 512;
const MAX_NAME: usize = 1_024;
/// Longest token sent, and the longest name of a refusal that is shown.
const MAX_TOKEN: usize = 16 * 1024;
const MAX_CODE: usize = 64;
/// How long the service has to answer one question, and all of them together.
const ASK_TIMEOUT: Duration = Duration::from_secs(30);
const READ_TIMEOUT: Duration = Duration::from_secs(90);
/// The longest the service is believed when it says how long to stay away (seconds).
const MAX_WAIT: u64 = 24 * 3_600;

/// The Favorites of the mailbox as classic Outlook has them, for the owner of `token` (an
/// access token for Exchange Web Services): the folders under Favorites in its Mail pane, in
/// that order, each with where it is, and how many shortcuts were left out.
/// [`FavoritesOutcome::Missing`] when the mailbox has no such list.
///
/// The download doesn't know whose it is: say so with [`FavoritesDownload::for_account`] before
/// the mailbox takes it in. Blocks for about a minute and a half at most. Only ever reads.
pub fn favorites(token: &str) -> Result<FavoritesDownload, NetError> {
    favorites_at(&endpoint(), token)
}

/// Where the service is spoken to: its own address, always. (Only a test may have a stand-in on
/// this machine answer in its place, on its own thread: `standin::route`.)
fn endpoint() -> String {
    #[cfg(any(test, feature = "stand-in"))]
    if let Some(base) = standin::routed() {
        return format!("{base}{PATH}");
    }
    ENDPOINT.to_string()
}

fn nonsense(text: &str) -> NetError {
    NetError::Proto(ProtoError::Protocol(text.into()))
}

/// A no from the service: the error to hand on, and whether it is about the way it was asked
/// (a property or a shape it won't have) rather than about who asked or what there is.
#[derive(Debug)]
struct Said {
    error: NetError,
    shape: bool,
}

impl From<NetError> for Said {
    fn from(error: NetError) -> Said {
        Said { error, shape: false }
    }
}

/// The name the service gives a refusal (`ErrorAccessDenied`), when what stands in its place
/// is a name: letters and digits, of a length a name has, and nothing of what was sent.
fn code_name(code: &str, token: &str) -> Option<String> {
    let code = code.trim().rsplit(':').next().unwrap_or("");
    let named = !code.is_empty() && code.len() <= MAX_CODE && code.bytes().all(|b| b.is_ascii_alphanumeric());
    (named && !token.contains(code)).then(|| code.to_string())
}

/// The error for a refusal the service names by `code` ([MS-OXWSCDATA], `ResponseCodeType`).
fn refusal(code: &str, token: &str) -> Said {
    let Some(code) = code_name(code, token) else {
        return nonsense("Exchange Web Services refused without saying why").into();
    };
    let error = match code.as_str() {
        "ErrorAccessDenied" => NetError::Forbidden("this account may not read its folders through Exchange Web Services (ErrorAccessDenied)".into()),
        "ErrorServerBusy"
        | "ErrorInternalServerTransientError"
        | "ErrorTimeoutExpired"
        | "ErrorMailboxStoreUnavailable"
        | "ErrorConnectionFailed" => NetError::Busy { text: "Exchange Web Services is busy; try again later".into(), retry_after: None },
        _ => NetError::Proto(ProtoError::Rejected(format!("Exchange Web Services answered {code}"))),
    };
    // A property it has no such type for, a shape it won't take, a request that isn't valid by
    // its schema: asking for less may still be answered.
    let shape = code == "ErrorSchemaValidation" || code.starts_with("ErrorInvalid") || code.starts_with("ErrorUnsupported");
    Said { error, shape }
}

/// What an `X-EWS-Policy-Reason` header says, in our own words: the two values Microsoft
/// documents ("Control access to EWS in Exchange"), and anything else as no more than that the
/// organisation's policy said no. The header's own text is never shown.
fn blocked(reason: &str) -> NetError {
    let reason = reason.to_ascii_lowercase();
    NetError::Blocked(
        if reason.contains("disabled") {
            "Exchange Web Services is switched off for this account by its organisation"
        } else if reason.contains("blocked by policy") {
            "this account's organisation doesn't let this program use Exchange Web Services"
        } else {
            "this account's organisation doesn't allow Exchange Web Services"
        }
        .into(),
    )
}

/// How many seconds a `Retry-After` header asks for, when it says so in seconds.
fn wait_of(retry_after: Option<&str>) -> Option<u32> {
    let seconds = retry_after?.trim().parse::<u64>().ok()?;
    u32::try_from(seconds.min(MAX_WAIT)).ok()
}

/// An id of the service's that may be sent back to it: the letters of base64, in either
/// alphabet, of a sane length. Anything else is not an id, and goes nowhere.
fn is_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_ID && id.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'=' | b'-' | b'_'))
}

/// One request: the operation, in an envelope that names the schema's version.
fn envelope(operation: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
         <soap:Envelope xmlns:soap=\"{SOAP}\" xmlns:t=\"{TYPES}\" xmlns:m=\"{MESSAGES}\">\
         <soap:Header><t:RequestServerVersion Version=\"{VERSION}\"/></soap:Header>\
         <soap:Body>{operation}</soap:Body>\
         </soap:Envelope>"
    )
}

/// `FindFolder` for the folder called Common Views, directly under the root of the mailbox.
fn find_common_views() -> String {
    format!(
        "<m:FindFolder Traversal=\"Shallow\">\
         <m:FolderShape><t:BaseShape>IdOnly</t:BaseShape>\
         <t:AdditionalProperties><t:FieldURI FieldURI=\"folder:DisplayName\"/></t:AdditionalProperties></m:FolderShape>\
         <m:IndexedPageFolderView MaxEntriesReturned=\"10\" Offset=\"0\" BasePoint=\"Beginning\"/>\
         <m:Restriction><t:IsEqualTo><t:FieldURI FieldURI=\"folder:DisplayName\"/>\
         <t:FieldURIOrConstant><t:Constant Value=\"{}\"/></t:FieldURIOrConstant></t:IsEqualTo></m:Restriction>\
         <m:ParentFolderIds><t:DistinguishedFolderId Id=\"root\"/></m:ParentFolderIds>\
         </m:FindFolder>",
        xml::escape(COMMON_VIEWS)
    )
}

/// An extended property by its tag, as it is asked for.
fn property(tag: u16, kind: &str) -> String {
    format!("<t:ExtendedFieldURI PropertyTag=\"0x{tag:04X}\" PropertyType=\"{kind}\"/>")
}

/// `FindItem` for a page of the shortcuts in the folder `views`, from the one at `offset`.
/// `grouped`: with the two properties that say which group a shortcut is in, each under both
/// of the types the documents give them ([`links`]).
fn find_links(views: &str, offset: usize, grouped: bool) -> String {
    let mut properties = [(links::TYPE, "Integer"), (links::SECTION, "Integer"), (links::ORDINAL, "Binary"), (links::ENTRY_ID, "Binary")]
        .map(|(tag, kind)| property(tag, kind))
        .concat();
    if grouped {
        for tag in [links::GROUP, links::GROUP_HEADER] {
            properties.push_str(&property(tag, "Binary"));
            properties.push_str(&property(tag, "CLSID"));
        }
    }
    format!(
        "<m:FindItem Traversal=\"Associated\">\
         <m:ItemShape><t:BaseShape>IdOnly</t:BaseShape><t:AdditionalProperties>{properties}</t:AdditionalProperties></m:ItemShape>\
         <m:IndexedPageItemView MaxEntriesReturned=\"{LINK_PAGE}\" Offset=\"{offset}\" BasePoint=\"Beginning\"/>\
         <m:Restriction><t:IsEqualTo><t:FieldURI FieldURI=\"item:ItemClass\"/>\
         <t:FieldURIOrConstant><t:Constant Value=\"{}\"/></t:FieldURIOrConstant></t:IsEqualTo></m:Restriction>\
         <m:ParentFolderIds><t:FolderId Id=\"{}\"/></m:ParentFolderIds>\
         </m:FindItem>",
        xml::escape(LINK_CLASS),
        xml::escape(views)
    )
}

/// `GetFolder` for the top of the mailbox and the special folders, in that order.
fn get_special() -> String {
    let ids: String =
        std::iter::once(TOP).chain(DISTINGUISHED.iter().map(|(id, _)| *id)).map(|id| format!("<t:DistinguishedFolderId Id=\"{id}\"/>")).collect();
    format!("<m:GetFolder><m:FolderShape><t:BaseShape>IdOnly</t:BaseShape></m:FolderShape><m:FolderIds>{ids}</m:FolderIds></m:GetFolder>")
}

/// `FindFolder` for a page of all the folders under the top of the mailbox, from `offset`.
fn find_tree(offset: usize) -> String {
    format!(
        "<m:FindFolder Traversal=\"Deep\">\
         <m:FolderShape><t:BaseShape>IdOnly</t:BaseShape><t:AdditionalProperties>\
         <t:FieldURI FieldURI=\"folder:DisplayName\"/><t:FieldURI FieldURI=\"folder:ParentFolderId\"/>{}\
         </t:AdditionalProperties></m:FolderShape>\
         <m:IndexedPageFolderView MaxEntriesReturned=\"{TREE_PAGE}\" Offset=\"{offset}\" BasePoint=\"Beginning\"/>\
         <m:ParentFolderIds><t:DistinguishedFolderId Id=\"{TOP}\"/></m:ParentFolderIds>\
         </m:FindFolder>",
        property(ENTRY_ID, "Binary")
    )
}

/// Whether a page says it is the last of its list (`IncludesLastItemInRange`). One that
/// doesn't say is taken for the last: there is no asking for what comes after nothing.
fn is_last(page: &Node) -> bool {
    !page.attribute("IncludesLastItemInRange").is_some_and(|last| last.trim().eq_ignore_ascii_case("false"))
}

/// The folders of the mailbox, by what the service calls them.
#[derive(Default)]
struct Tree {
    /// What each folder is called, and the folder it lies in.
    known: HashMap<String, (String, String)>,
    /// Which folder an EntryID names ([`links::key_of`]).
    by_entry: HashMap<Vec<u8>, String>,
}

/// The service as one caller has it: where it is, the token that says who is asking, and when
/// the asking began.
struct Ews<'a> {
    url: Url,
    token: &'a str,
    started: Instant,
}

impl<'a> Ews<'a> {
    fn at(endpoint: &str, token: &'a str) -> Result<Ews<'a>, NetError> {
        // A token that isn't one is not sent anywhere; it is also what a header can't carry.
        if token.is_empty() || token.len() > MAX_TOKEN || !token.bytes().all(|b| b.is_ascii_graphic()) {
            return Err(NetError::Proto(ProtoError::Auth("there is no sign-in to show Exchange Web Services".into())));
        }
        Ok(Ews { url: Url::parse(endpoint)?, token, started: Instant::now() })
    }

    /// One operation: the answers to it (`{operation}ResponseMessage`, one for each thing that
    /// was asked about), whatever each says.
    fn ask(&self, operation: &str, body: &str) -> Result<Vec<Node>, Said> {
        let left = READ_TIMEOUT
            .checked_sub(self.started.elapsed())
            .filter(|left| !left.is_zero())
            .ok_or_else(|| NetError::Proto(ProtoError::Io("Exchange Web Services took too long to answer".into())))?;
        let bearer = format!("Bearer {}", self.token);
        let headers = [("Authorization", bearer.as_str()), ("Content-Type", "text/xml; charset=utf-8"), ("Accept", "text/xml")];
        let request =
            Request { method: "POST", headers: &headers, body: Some(&envelope(body)), patience: left.min(ASK_TIMEOUT), max_body: MAX_ANSWER };
        let reply = http::request(&self.url, &request).map_err(|e| e.without(self.token))?;
        self.read(operation, &reply)
    }

    /// What a reply to `operation` holds, or what it refuses with.
    fn read(&self, operation: &str, reply: &Reply) -> Result<Vec<Node>, Said> {
        match reply.status {
            200 => {}
            401 => return Err(NetError::Proto(ProtoError::Auth("Exchange Web Services didn't take the sign-in".into())).into()),
            // The header says whose no it is; the body is not looked at.
            403 => {
                return Err(match &reply.policy {
                    Some(reason) => blocked(reason),
                    None => NetError::Forbidden("this account may not use Exchange Web Services".into()),
                }
                .into());
            }
            429 | 503 | 504 => {
                let text = "Exchange Web Services is busy; try again later".to_string();
                return Err(NetError::Busy { text, retry_after: wait_of(reply.retry_after.as_deref()) }.into());
            }
            // A fault: what the request itself was refused for, when the answer is one.
            500 => {
                let fault = xml::parse(&reply.body).ok().and_then(|root| root.find(&["Body", "Fault"]).map(fault_code));
                return Err(match fault {
                    Some(code) => refusal(&code, self.token),
                    None => NetError::Proto(ProtoError::Rejected("Exchange Web Services answered with status 500".into())).into(),
                });
            }
            status => return Err(NetError::Proto(ProtoError::Rejected(format!("Exchange Web Services answered with status {status}"))).into()),
        }
        let unreadable = || nonsense("Exchange Web Services' answer can't be read");
        let mut root = xml::parse(&reply.body).map_err(|_| unreadable())?;
        if root.name != "Envelope" {
            return Err(unreadable().into());
        }
        let body = root.children.iter_mut().find(|child| child.name == "Body").ok_or_else(unreadable)?;
        if let Some(fault) = body.child("Fault") {
            return Err(refusal(&fault_code(fault), self.token));
        }
        let response = body.children.iter_mut().find(|child| child.name == format!("{operation}Response")).ok_or_else(unreadable)?;
        let messages = response.children.iter_mut().find(|child| child.name == "ResponseMessages").ok_or_else(unreadable)?;
        let wanted = format!("{operation}ResponseMessage");
        let answers: Vec<Node> = std::mem::take(&mut messages.children).into_iter().filter(|message| message.name == wanted).collect();
        if answers.is_empty() { Err(unreadable().into()) } else { Ok(answers) }
    }

    /// The one answer to an operation that asks about one thing, when it is a yes.
    fn one(&self, operation: &str, body: &str) -> Result<Node, Said> {
        let mut answers = self.ask(operation, body)?;
        // (More answers than questions is not the answer to this one.)
        let (Some(answer), true) = (answers.pop(), answers.is_empty()) else {
            return Err(nonsense("Exchange Web Services' answer can't be read").into());
        };
        accepted(&answer, self.token)?;
        Ok(answer)
    }

    /// The id of the Common Views folder; `None` when the mailbox has none.
    fn common_views(&self) -> Result<Option<String>, Said> {
        let answer = self.one("FindFolder", &find_common_views())?;
        let folders = answer.find(&["RootFolder", "Folders"]).map(|folders| folders.children.as_slice()).unwrap_or_default();
        // (Whatever else the service thought fit to answer with: only a folder of that name.)
        let named = folders.iter().take(64).find(|folder| folder.text_of("DisplayName") == COMMON_VIEWS);
        match named.and_then(|folder| folder.child("FolderId")).and_then(|id| id.attribute("Id")) {
            Some(id) if is_id(id) => Ok(Some(id.to_string())),
            Some(_) => Err(nonsense("Exchange Web Services named a folder in a way that can't be used").into()),
            None => Ok(None),
        }
    }

    /// Every shortcut in the folder `views`, page by page to the end.
    fn links(&self, views: &str, grouped: bool) -> Result<Vec<Link>, Said> {
        let mut all: Vec<Link> = Vec::new();
        for _ in 0..MAX_LINK_PAGES {
            let answer = self.one("FindItem", &find_links(views, all.len(), grouped))?;
            let page = answer.child("RootFolder").ok_or_else(|| nonsense("Exchange Web Services' answer can't be read"))?;
            let items = page.child("Items").map(|items| items.children.as_slice()).unwrap_or_default();
            all.extend(items.iter().take(LINK_PAGE).map(links::link_of));
            // (A page with nothing on it ends the list whatever it says: the next would be it again.)
            if is_last(page) || items.is_empty() {
                return Ok(all);
            }
            if all.len() >= links::MAX_LINKS {
                break;
            }
        }
        // More of them than anybody has: what was read is not the list.
        Err(nonsense("the list of Favorites on the server is longer than is read").into())
    }

    /// The top of the mailbox, and which of its folders is which special folder, by their ids.
    fn special(&self) -> Result<(String, HashMap<String, FolderKind>), Said> {
        let answers = self.ask("GetFolder", &get_special())?;
        // An answer for each folder that was asked about, in that order: nothing else says
        // which is which.
        if answers.len() != DISTINGUISHED.len() + 1 {
            return Err(nonsense("Exchange Web Services' answer can't be read").into());
        }
        let id_of = |answer: &Node| -> Result<Option<String>, Said> {
            match accepted(answer, self.token) {
                Ok(()) => {}
                // (A mailbox that has no such folder: it just isn't one of them.)
                Err(said) if matches!(&said.error, NetError::Proto(ProtoError::Rejected(_))) => return Ok(None),
                Err(said) => return Err(said),
            }
            let folder = answer.child("Folders").and_then(|folders| folders.children.first());
            Ok(folder.and_then(|folder| folder.child("FolderId")).and_then(|id| id.attribute("Id")).filter(|id| is_id(id)).map(str::to_string))
        };
        let mut answers = answers.iter();
        let top = answers.next().map(id_of).transpose()?.flatten();
        let Some(top) = top else { return Err(nonsense("Exchange Web Services didn't say where the mailbox begins").into()) };
        let mut kinds: HashMap<String, FolderKind> = HashMap::new();
        for (answer, (_, kind)) in answers.zip(DISTINGUISHED) {
            if let Some(id) = id_of(answer)? {
                kinds.entry(id).or_insert(kind);
            }
        }
        Ok((top, kinds))
    }

    /// Every folder under the top of the mailbox, page by page. Whether that was all of them.
    fn tree(&self) -> Result<(Tree, bool), Said> {
        let mut tree = Tree::default();
        let mut read = 0usize;
        for _ in 0..MAX_TREE_PAGES {
            let answer = self.one("FindFolder", &find_tree(read))?;
            let page = answer.child("RootFolder").ok_or_else(|| nonsense("Exchange Web Services' answer can't be read"))?;
            let folders = page.child("Folders").map(|folders| folders.children.as_slice()).unwrap_or_default();
            for folder in folders.iter().take(TREE_PAGE) {
                read += 1;
                let id = folder.child("FolderId").and_then(|id| id.attribute("Id")).unwrap_or("");
                let parent = folder.child("ParentFolderId").and_then(|id| id.attribute("Id")).unwrap_or("");
                // A folder that can't be named is passed over: nothing can lie in it either.
                if !is_id(id) || !(parent.is_empty() || is_id(parent)) {
                    continue;
                }
                let name: String = folder.text_of("DisplayName").chars().take(MAX_NAME).collect();
                let entry = folder.all("ExtendedProperty").take(8).find_map(|property| {
                    let tag = property.child("ExtendedFieldURI").and_then(|uri| uri.attribute("PropertyTag")).and_then(links::tag_of);
                    (tag == Some(ENTRY_ID)).then(|| links::binary(property.text_of("Value"), MAX_ID)).flatten()
                });
                if let Some(entry) = entry {
                    tree.by_entry.entry(links::key_of(&entry).to_vec()).or_insert_with(|| id.to_string());
                }
                tree.known.entry(id.to_string()).or_insert_with(|| (name, parent.to_string()));
            }
            if is_last(page) || folders.is_empty() {
                return Ok((tree, true));
            }
        }
        Ok((tree, false))
    }
}

/// The name a SOAP fault gives its refusal: the `ResponseCode` in its detail, else its
/// `faultcode` (`a:ErrorSchemaValidation`, of which the name is what follows the prefix).
fn fault_code(fault: &Node) -> String {
    let detail = fault.find(&["detail", "ResponseCode"]).map(|code| code.text.trim()).filter(|code| !code.is_empty());
    detail.unwrap_or_else(|| fault.text_of("faultcode")).to_string()
}

/// Whether an answer is a yes (`ResponseClass="Success"`, or a warning, which still answers);
/// the refusal it names otherwise.
fn accepted(answer: &Node, token: &str) -> Result<(), Said> {
    match answer.attribute("ResponseClass").map(str::trim) {
        Some("Success" | "Warning") => Ok(()),
        Some("Error") => Err(refusal(answer.text_of("ResponseCode"), token)),
        _ => Err(nonsense("Exchange Web Services' answer can't be read").into()),
    }
}

/// [`favorites`] from the service at `endpoint`.
fn favorites_at(endpoint: &str, token: &str) -> Result<FavoritesDownload, NetError> {
    let ews = Ews::at(endpoint, token)?;
    let missing = || Ok(FavoritesDownload { outcome: FavoritesOutcome::Missing, ..Default::default() });
    let Some(views) = ews.common_views().map_err(|said| said.error)? else {
        log::info!("favorites (EWS): the mailbox has no folder for them");
        return missing();
    };
    // With the groups first; should the service not have it asked that way, without.
    let links = match ews.links(&views, true) {
        Err(said) if said.shape => {
            log::info!("favorites (EWS): the service won't be asked for the groups; asking without them");
            ews.links(&views, false)
        }
        outcome => outcome,
    }
    .map_err(|said| said.error)?;
    let wanted = links::wanted(&links);
    // Nothing at all under the Favorites of Mail: classic Outlook never kept a list here.
    if !links.iter().any(|link| link.section == Some(1)) {
        log::info!("favorites (EWS): {} shortcuts on the server, none of them of the Favorites of Mail", links.len());
        return missing();
    }
    let mut download =
        FavoritesDownload { outcome: FavoritesOutcome::Read, skipped: wanted.skipped, unresolved: wanted.unresolved, ..Default::default() };
    let (mut folders, mut whole) = (0usize, true);
    if !wanted.folders.is_empty() {
        let (top, kinds) = ews.special().map_err(|said| said.error)?;
        let (tree, all) = ews.tree().map_err(|said| said.error)?;
        (folders, whole) = (tree.known.len(), all);
        let kind_of = |id: &str| kinds.get(id).copied().unwrap_or(FolderKind::Custom);
        let mut kept: Vec<&str> = Vec::new();
        for entry in &wanted.folders {
            // A shortcut to a folder that is gone, or into another store, names none of these.
            let found = tree.by_entry.get(links::key_of(entry)).and_then(|id| tree.known.get_key_value(id));
            let favorite = found.filter(|(id, _)| !kept.contains(&id.as_str()) && kept.len() < MAX_FAVORITES).and_then(|(id, (name, parent))| {
                kept.push(id.as_str());
                crate::favorite_paths::favorite_of(id, name, parent, &tree.known, Some(top.as_str()), &kind_of)
            });
            match favorite {
                Some(favorite) => download.folders.push(favorite),
                None => download.unresolved = download.unresolved.saturating_add(1),
            }
        }
    }
    log::info!(
        "favorites (EWS): {} shortcuts on the server, {} to folders of the Favorites of Mail, {} are folders of this mailbox, {} are not, {} are to something else ({folders} folders listed, all of them: {whole})",
        links.len(),
        wanted.folders.len(),
        download.folders.len(),
        download.unresolved,
        download.skipped
    );
    Ok(download)
}

#[cfg(test)]
mod tests;
