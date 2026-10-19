//! A mailbox kept in step over JMAP (RFC 8620 and 8621): what a run is asked to do and what it
//! brings back.
//!
//! The counterpart of [`GraphMailRequest`](super::GraphMailRequest) and
//! [`GraphMailResult`](super::GraphMailResult), beside them rather than bent into them. Plain data,
//! so the pieces stay apart: the engine builds a [`JmapMailRequest`] and takes in a
//! [`JmapMailResult`] (both pure), and a host with a way to reach the server (`fetch` in the
//! browser, an HTTP client where there are sockets) does the talking in between, usually as a task
//! that hands the answer over in pieces.
//!
//! A JMAP account's folders and messages are known to its server by the ids the server hands out
//! ([`crate::AccountKind::server_ids`]), as a Microsoft account's are. What carries no Graph
//! meaning is used again here instead of being copied: [`FolderListing`], [`MessagePage`],
//! [`SendUp`], [`SendDone`], [`GraphProgress`] and, for a message fetched whole, what that brings
//! ([`GraphFetchResult`](super::GraphFetchResult)). What is asked for it is a [`JmapFetchRequest`]
//! of its own, since a host needs to know where the server is.
//!
//! A run reads a **window** of each mailbox it reads: the newest messages, [`JmapMailRequest::window`]
//! of them. A mailbox with no more messages than that is read whole, and its page says so
//! ([`MessagePage::afresh`]); a bigger one is read in part, and what is not in the window is
//! neither added nor removed by it.
//!
//! Neither type shows the server's address, the account's address or a message in debug output:
//! counts only. The secret that opens the server (a password, or a bearer token) is handed to the
//! host beside the request and is in neither.
//!
//! Saving drafts on the server and the Drafts that go up with a run are not here yet: a `drafts`
//! list joins the request and the result when that is built. Both read a missing field as empty,
//! so the files and answers written until then stay good.

use serde::{Deserialize, Serialize};

use super::{FolderListing, GraphProgress, MessagePage, SendDone, SendUp, is_false};
use crate::Id;

/// Most messages of one mailbox that a run reads (the newest ones): what [`JmapMailRequest::window`]
/// is held to, here and by the host.
pub const MAX_JMAP_WINDOW: u32 = 500;

/// What a run for a [`crate::AccountKind::Jmap`] account is asked to do: the changes made here go
/// up, what waits in the Outbox is sent, the server's mailboxes are listed if they should be, and
/// the newest messages of the mailboxes in `scope` are read.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct JmapMailRequest {
    pub account: Id,
    /// The account's address: what the answer is checked against, and the mailbox whose requests
    /// are counted together.
    pub email: String,
    /// The address of the server's session resource ([`crate::JmapServer::session`]). It may carry
    /// the account's address, so it is never logged.
    pub session: String,
    /// The secret that goes beside this request is a bearer token. Otherwise it is the account's
    /// password, sent with the account's address as its login name ([`crate::JmapServer::bearer`]).
    #[serde(skip_serializing_if = "is_false")]
    pub bearer: bool,
    /// How many of the newest messages of a mailbox are read at most, from 1 to [`MAX_JMAP_WINDOW`].
    pub window: u32,
    /// List the server's mailboxes in this run: the first run, a run that reads every mailbox, and
    /// the run after one that brought no listing.
    #[serde(skip_serializing_if = "is_false")]
    pub list_folders: bool,
    /// Which mailboxes are read.
    pub scope: JmapScope,
    /// The messages each mailbox has here, so that the host asks for the whole of only those it
    /// hasn't got, and for the marks alone of the rest. A mailbox that isn't here has none.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub known: Vec<JmapKnown>,
    /// The changes of messages made here, after which the mailboxes are read.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub changes: Vec<JmapChange>,
    /// What waits in the Outbox, sent after the changes (RFC 8621 §7). Each message goes whole,
    /// as its `mime`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sends: Vec<SendUp>,
    /// Nobody is waiting for this run: it gets the share of requests background work has.
    #[serde(skip_serializing_if = "is_false")]
    pub background: bool,
}

/// Messages of a JMAP account to fetch whole (their raw octets, which an email's blob holds), by
/// server id, in this order: the counterpart of [`GraphFetchRequest`](super::GraphFetchRequest).
///
/// It says where the server is, as a [`JmapMailRequest`] does, because a host that has to connect
/// for it has no other way to know. What it brings back is the same [`GraphFetchResult`](super::GraphFetchResult)
/// a Microsoft account's fetch brings, so that taking it in is the same for both. The secret that
/// opens the server goes beside the request, as for a run, and is not in it.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct JmapFetchRequest {
    pub account: Id,
    /// The account's address: what the answer is checked against.
    pub email: String,
    /// The address of the server's session resource ([`crate::JmapServer::session`]). It may carry
    /// the account's address, so it is never logged.
    pub session: String,
    /// The secret that goes beside this request is a bearer token. Otherwise it is the account's
    /// password, sent with the account's address as its login name ([`crate::JmapServer::bearer`]).
    #[serde(skip_serializing_if = "is_false")]
    pub bearer: bool,
    /// The server ids of the emails to fetch, in the order they are wanted.
    pub ids: Vec<String>,
    /// Nobody is waiting: it gets the share of requests background work has.
    #[serde(skip_serializing_if = "is_false")]
    pub background: bool,
}

/// Which mailboxes a run reads.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum JmapScope {
    /// The Inbox and the mailboxes named (the server ids of those on screen): the cheap run that
    /// keeps what the owner is looking at fresh.
    Light {
        #[serde(default)]
        also: Vec<String>,
    },
    /// Every mailbox of the listing.
    #[default]
    All,
}

impl JmapScope {
    /// A word for it that names no mailbox: `all`, or `light` and how many are named besides the
    /// Inbox.
    fn describe(&self) -> String {
        match self {
            JmapScope::All => "all".into(),
            JmapScope::Light { also } => format!("light+{}", also.len()),
        }
    }
}

/// The messages one mailbox has here, as the server's ids for them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct JmapKnown {
    /// The mailbox's server id.
    pub mailbox: String,
    /// The server ids of its messages that are here, newest first. The engine sends the newest
    /// ones, and no more than a host can use.
    pub ids: Vec<String>,
}

/// A change made here to a message of a JMAP account, for its server to take on: worked out from
/// the difference between the message and what the server last said of it
/// ([`ServerMessage`](super::ServerMessage)), or waiting in [`crate::Mailbox::queued`]. Messages
/// are named by their server ids, mailboxes by theirs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum JmapChange {
    /// The marks of the message `message` (the server's `id`) were changed here: only those are
    /// named, and they go as one change. `read` is `$seen`, `flagged` is `$flagged`, `answered`
    /// is `$answered` and `forwarded` is `$forwarded`; other keywords are never touched.
    #[serde(rename_all = "camelCase")]
    Keywords {
        message: Id,
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        read: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        flagged: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        answered: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        forwarded: Option<bool>,
    },
    /// The message lies in another mailbox here: `to` instead of `from`. Some servers give a
    /// message a new id when it moves ([`JmapOutcome::Done`] says it).
    Move { message: Id, id: String, from: String, to: String },
    /// The message was deleted for good here: the server destroys it.
    Destroy { id: String },
}

impl JmapChange {
    /// The server id of the message it is about.
    pub fn id(&self) -> &str {
        match self {
            JmapChange::Keywords { id, .. } | JmapChange::Move { id, .. } | JmapChange::Destroy { id } => id,
        }
    }
}

/// What the server made of a [`JmapChange`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum JmapOutcome {
    /// Done. `id`: the message's new server id, when the server said one (a move can give it a
    /// new id); empty when it didn't, or when the id stays.
    Done {
        #[serde(default, skip_serializing_if = "String::is_empty")]
        id: String,
    },
    /// The server no longer has the message.
    Gone,
    /// The server won't: `why` says so in plain words.
    Refused { why: String },
}

/// A change and what became of it. A change that isn't answered (the server was busy, the
/// connection went) isn't here: it is worked out again for the next run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JmapChangeDone {
    pub change: JmapChange,
    pub outcome: JmapOutcome,
}

/// What a run for a JMAP account brought back, or a piece of it.
///
/// A run arrives in pieces, in the order things were done (`partial` on all but the last), each
/// whole in itself: taking the pieces in one after the other, as far as they come, leaves the
/// mailbox as far along as the run got.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct JmapMailResult {
    pub account: Id,
    /// The address of the account the run was for: an answer is not taken in by another account
    /// that happens to have the id.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub email: String,
    /// What became of the changes of messages sent.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub changes: Vec<JmapChangeDone>,
    /// What became of the messages sent.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sent: Vec<SendDone>,
    /// Every mailbox of the server, when it could be listed at all.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub listing: Option<FolderListing>,
    /// The windows read, in the order they came: each is taken in by itself.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub pages: Vec<MessagePage>,
    /// Problems worth telling the owner; empty when all went well.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
    /// The server said it was busy and to stay away this many seconds: all work for the account
    /// waits that long.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub busy_for: Option<u32>,
    /// More of this run comes after this piece.
    #[serde(skip_serializing_if = "is_false")]
    pub partial: bool,
    /// Nothing went wrong in the run and it was not stopped: every mailbox it meant to read was read,
    /// if in part. A window that does not hold all of its mailbox marks nothing gone (only a page with
    /// [`MessagePage::afresh`] does), so what the pages left out of the windows that do is gone from
    /// those mailboxes (deleted, or moved to a mailbox this run did not read), and the engine removes it now.
    #[serde(skip_serializing_if = "is_false")]
    pub whole: bool,
    /// The owner stopped the run (Cancel) after the piece in hand: the next one goes on from there.
    #[serde(skip_serializing_if = "is_false")]
    pub stopped: bool,
    /// How far the run has got, for the status bar.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<GraphProgress>,
    /// The server turned the secret down (the host could not get another that it accepts): the
    /// owner is told to sign in again, and the run is not repeated by itself.
    #[serde(skip_serializing_if = "is_false")]
    pub auth_refused: bool,
}

impl JmapMailResult {
    /// Take in the next piece of the same run, for a caller that takes a run in at once: the pages
    /// and the rest in the order they came, and what the last piece says of the end.
    pub fn absorb(&mut self, piece: JmapMailResult) {
        self.account = piece.account;
        if !piece.email.is_empty() {
            self.email = piece.email;
        }
        self.changes.extend(piece.changes);
        self.sent.extend(piece.sent);
        if piece.listing.is_some() {
            self.listing = piece.listing;
        }
        self.pages.extend(piece.pages);
        self.errors.extend(piece.errors);
        self.busy_for = piece.busy_for.or(self.busy_for);
        (self.partial, self.whole, self.stopped) = (piece.partial, piece.whole, piece.stopped);
        if piece.progress.is_some() {
            self.progress = piece.progress;
        }
        self.auth_refused |= piece.auth_refused;
    }
}

/// Counts only: the server's address, the account's, and every id, name and word of a message stay
/// out of debug output.
impl std::fmt::Debug for JmapMailRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let known: usize = self.known.iter().map(|k| k.ids.len()).sum();
        f.debug_struct("JmapMailRequest")
            .field("account", &self.account)
            .field("bearer", &self.bearer)
            .field("window", &self.window)
            .field("list_folders", &self.list_folders)
            .field("scope", &self.scope.describe())
            .field("mailboxes", &self.known.len())
            .field("known", &known)
            .field("changes", &self.changes.len())
            .field("sends", &self.sends.len())
            .field("background", &self.background)
            .finish()
    }
}

/// Counts only, as a run's request: no address, and no id of a message.
impl std::fmt::Debug for JmapFetchRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JmapFetchRequest")
            .field("account", &self.account)
            .field("bearer", &self.bearer)
            .field("ids", &self.ids.len())
            .field("background", &self.background)
            .finish()
    }
}

/// Counts only, as the request's.
impl std::fmt::Debug for JmapMailResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let messages: usize = self.pages.iter().map(|page| page.messages.len()).sum();
        f.debug_struct("JmapMailResult")
            .field("account", &self.account)
            .field("changes", &self.changes.len())
            .field("sent", &self.sent.len())
            .field("listed", &self.listing.as_ref().map(|listing| listing.folders.len()))
            .field("pages", &self.pages.len())
            .field("messages", &messages)
            .field("errors", &self.errors.len())
            .field("busy_for", &self.busy_for)
            .field("partial", &self.partial)
            .field("whole", &self.whole)
            .field("stopped", &self.stopped)
            .field("progress", &self.progress.is_some())
            .field("auth_refused", &self.auth_refused)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::{GraphFolder, GraphMessage, SendOutcome, ServerMessage};
    use crate::{Address, FolderKind, Message};

    const ME: &str = "ada@example.org";
    const SESSION: &str = "https://mail.example.org/jmap/session?account=ada@example.org";

    fn round_trip<T: Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug>(value: &T) -> String {
        let json = serde_json::to_string(value).unwrap();
        assert_eq!(&serde_json::from_str::<T>(&json).unwrap(), value, "{json}");
        json
    }

    /// A request with something in every part of it, and the address everywhere it could be.
    fn request() -> JmapMailRequest {
        let to_bob = Message { id: 7, to: vec![Address::new("Bob", "bob@example.org")], subject: format!("For {ME}"), ..Default::default() };
        JmapMailRequest {
            account: 3,
            email: ME.into(),
            session: SESSION.into(),
            bearer: true,
            window: 300,
            list_folders: true,
            scope: JmapScope::Light { also: vec![format!("M-{ME}"), "M2".into()] },
            known: vec![
                JmapKnown { mailbox: "M1".into(), ids: vec![format!("E-{ME}"), "E2".into(), "E3".into()] },
                JmapKnown { mailbox: "M2".into(), ids: vec![] },
            ],
            changes: vec![
                JmapChange::Keywords { message: 4, id: "E2".into(), read: Some(true), flagged: None, answered: Some(false), forwarded: None },
                JmapChange::Move { message: 5, id: "E3".into(), from: "M1".into(), to: "M2".into() },
                JmapChange::Destroy { id: "E9".into() },
            ],
            sends: vec![SendUp { message: 7, draft: None, mime: Some(Box::new(to_bob)) }],
            background: true,
        }
    }

    fn result() -> JmapMailResult {
        let message = GraphMessage {
            server: ServerMessage { id: "E2".into(), folder: "M1".into(), read: true, answered: true, forwarded: true, ..Default::default() },
            message_id: "a@mail.example.org".into(),
            subject: format!("Hello {ME}"),
            from: Address::new("Ada", ME),
            to: vec![Address::new("Bob", "bob@example.org")],
            preview: format!("To {ME}"),
            ..Default::default()
        };
        JmapMailResult {
            account: 3,
            email: ME.into(),
            changes: vec![
                JmapChangeDone { change: JmapChange::Destroy { id: "E9".into() }, outcome: JmapOutcome::Done { id: String::new() } },
                JmapChangeDone {
                    change: JmapChange::Move { message: 5, id: "E3".into(), from: "M1".into(), to: "M2".into() },
                    outcome: JmapOutcome::Done { id: "E3b".into() },
                },
                JmapChangeDone { change: JmapChange::Destroy { id: "E8".into() }, outcome: JmapOutcome::Gone },
                JmapChangeDone {
                    change: JmapChange::Destroy { id: "E7".into() },
                    outcome: JmapOutcome::Refused { why: format!("{ME} may not delete it") },
                },
            ],
            sent: vec![SendDone { message: 7, outcome: SendOutcome::Held { lost: false, why: format!("{ME} was refused") }, draft: None }],
            listing: Some(FolderListing {
                folders: vec![GraphFolder { id: "M1".into(), name: ME.into(), kind: FolderKind::Inbox, total: 4, unread: 1, ..Default::default() }],
                complete: true,
            }),
            pages: vec![MessagePage { folder: "M1".into(), messages: vec![message], afresh: true, ..Default::default() }],
            errors: vec![format!("no mailbox for {ME}")],
            busy_for: Some(30),
            partial: true,
            whole: true,
            stopped: true,
            progress: Some(GraphProgress { folder: "M1".into(), read: 2, total: 4, number: 1, folders: 3 }),
            auth_refused: true,
        }
    }

    #[test]
    fn a_request_and_a_result_read_and_write_as_json() {
        let json = round_trip(&request());
        for part in [r#""scope":{"light":{"also":["#, r#""known":[{"mailbox":"M1","ids":["#, r#""sends":[{"message":7,"mime":"#, r#""bearer":true"#] {
            assert!(json.contains(part), "{part} in {json}");
        }
        let json = round_trip(&result());
        for part in [r#""authRefused":true"#, r#""busyFor":30"#, r#""whole":true"#, r#""afresh":true"#, r#""stopped":true"#] {
            assert!(json.contains(part), "{part} in {json}");
        }
        assert!(!json.contains("auth_refused") && !json.contains("busy_for"), "{json}");
    }

    #[test]
    fn what_is_not_said_is_empty() {
        let none: JmapMailRequest = serde_json::from_str("{}").unwrap();
        assert_eq!(none, JmapMailRequest::default());
        assert_eq!((none.window, none.scope.clone(), none.known.len(), none.changes.len(), none.sends.len()), (0, JmapScope::All, 0, 0, 0));
        let none: JmapMailResult = serde_json::from_str("{}").unwrap();
        assert_eq!(none, JmapMailResult::default());
        // An empty run writes next to nothing, and reads back as it was.
        assert_eq!(round_trip(&JmapMailResult::default()), "{\"account\":0}");
        assert_eq!(round_trip(&JmapMailRequest::default()), r#"{"account":0,"email":"","session":"","window":0,"scope":"all"}"#);
        // A result with only the parts that came: the others are empty.
        let some: JmapMailResult = serde_json::from_str(r#"{"account": 3, "pages": [{"folder": "M1"}], "whole": true}"#).unwrap();
        assert_eq!((some.account, some.pages.len(), some.whole, some.partial, some.listing.is_none()), (3, 1, true, false, true));
    }

    #[test]
    fn a_fetch_request_reads_and_writes_as_json_and_shows_counts_only() {
        let request = JmapFetchRequest {
            account: 3,
            email: ME.into(),
            session: SESSION.into(),
            bearer: true,
            ids: vec![format!("E-{ME}"), "E2".into()],
            background: true,
        };
        let json = round_trip(&request);
        for part in [r#""session":"https://mail.example.org/"#, r#""bearer":true"#, r#""ids":["E-ada@example.org","E2"]"#, r#""background":true"#] {
            assert!(json.contains(part), "{part} in {json}");
        }
        // What is false is not written, and what is not said reads as empty.
        let plain = JmapFetchRequest { account: 3, ids: vec!["E1".into()], ..Default::default() };
        assert_eq!(round_trip(&plain), r#"{"account":3,"email":"","session":"","ids":["E1"]}"#);
        assert_eq!(serde_json::from_str::<JmapFetchRequest>("{}").unwrap(), JmapFetchRequest::default());
        // Neither the account's address, nor the server's, nor a message's id is printed.
        for shown in [format!("{request:?}"), format!("{request:#?}")] {
            assert!(!shown.contains('@') && !shown.contains("example.org") && !shown.contains("E2") && !shown.contains("session"), "{shown}");
        }
        assert_eq!(format!("{request:?}"), "JmapFetchRequest { account: 3, bearer: true, ids: 2, background: true }");
    }

    #[test]
    fn a_scope_is_a_word_or_a_list() {
        assert_eq!(round_trip(&JmapScope::All), r#""all""#);
        assert_eq!(round_trip(&JmapScope::Light { also: vec!["M1".into()] }), r#"{"light":{"also":["M1"]}}"#);
        assert_eq!(serde_json::from_str::<JmapScope>(r#"{"light":{}}"#).unwrap(), JmapScope::Light { also: vec![] });
        assert!(serde_json::from_str::<JmapScope>(r#""everything""#).is_err());
        assert_eq!(JmapScope::default(), JmapScope::All);
    }

    #[test]
    fn a_change_names_only_what_changed() {
        let keywords = JmapChange::Keywords { message: 4, id: "E2".into(), read: Some(true), flagged: None, answered: None, forwarded: Some(false) };
        assert_eq!(round_trip(&keywords), r#"{"keywords":{"message":4,"id":"E2","read":true,"forwarded":false}}"#);
        let moved = JmapChange::Move { message: 5, id: "E3".into(), from: "M1".into(), to: "M2".into() };
        assert_eq!(round_trip(&moved), r#"{"move":{"message":5,"id":"E3","from":"M1","to":"M2"}}"#);
        let destroy = JmapChange::Destroy { id: "E9".into() };
        assert_eq!(round_trip(&destroy), r#"{"destroy":{"id":"E9"}}"#);
        assert_eq!([&keywords, &moved, &destroy].map(JmapChange::id), ["E2", "E3", "E9"]);
        // A change without the marks it names reads as one that names none.
        let bare: JmapChange = serde_json::from_str(r#"{"keywords":{"message":4,"id":"E2"}}"#).unwrap();
        assert_eq!(bare, JmapChange::Keywords { message: 4, id: "E2".into(), read: None, flagged: None, answered: None, forwarded: None });
        assert!(serde_json::from_str::<JmapChange>(r#"{"explode":{"id":"E2"}}"#).is_err());
        assert!(serde_json::from_str::<JmapChange>(r#"{"move":{"message":5,"id":"E3"}}"#).is_err());
    }

    #[test]
    fn an_outcome_is_done_gone_or_refused() {
        assert_eq!(round_trip(&JmapOutcome::Done { id: String::new() }), r#"{"done":{}}"#);
        assert_eq!(round_trip(&JmapOutcome::Done { id: "E3b".into() }), r#"{"done":{"id":"E3b"}}"#);
        assert_eq!(round_trip(&JmapOutcome::Gone), r#""gone""#);
        assert_eq!(round_trip(&JmapOutcome::Refused { why: "not allowed".into() }), r#"{"refused":{"why":"not allowed"}}"#);
        let done = JmapChangeDone { change: JmapChange::Destroy { id: "E9".into() }, outcome: JmapOutcome::Gone };
        assert_eq!(round_trip(&done), r#"{"change":{"destroy":{"id":"E9"}},"outcome":"gone"}"#);
        assert_eq!(round_trip(&JmapKnown { mailbox: "M1".into(), ids: vec!["E1".into()] }), r#"{"mailbox":"M1","ids":["E1"]}"#);
        assert_eq!(serde_json::from_str::<JmapKnown>("{}").unwrap(), JmapKnown::default());
    }

    #[test]
    fn the_pieces_of_a_run_come_together() {
        let page = |folder: &str, ids: &[&str]| MessagePage {
            folder: folder.into(),
            messages: ids
                .iter()
                .map(|id| GraphMessage { server: ServerMessage { id: (*id).into(), ..Default::default() }, ..Default::default() })
                .collect(),
            ..Default::default()
        };
        let pieces = [
            JmapMailResult {
                account: 3,
                email: ME.into(),
                partial: true,
                listing: Some(FolderListing { folders: vec![GraphFolder { id: "M1".into(), ..Default::default() }], complete: true }),
                changes: vec![JmapChangeDone { change: JmapChange::Destroy { id: "E9".into() }, outcome: JmapOutcome::Gone }],
                errors: vec!["One".into()],
                progress: Some(GraphProgress { folder: "M1".into(), read: 0, total: 2, number: 1, folders: 2 }),
                ..Default::default()
            },
            JmapMailResult {
                account: 3,
                partial: true,
                pages: vec![page("M1", &["E1", "E2"])],
                busy_for: Some(30),
                auth_refused: true,
                ..Default::default()
            },
            // (Nothing says the address here: it stays.)
            JmapMailResult {
                account: 3,
                pages: vec![page("M2", &["E3"])],
                errors: vec!["Two".into()],
                whole: true,
                stopped: true,
                ..Default::default()
            },
        ];
        let mut whole = JmapMailResult::default();
        pieces.into_iter().for_each(|piece| whole.absorb(piece));
        assert_eq!((whole.account, whole.email.as_str()), (3, ME));
        assert_eq!((whole.partial, whole.whole, whole.stopped, whole.auth_refused, whole.busy_for), (false, true, true, true, Some(30)));
        assert_eq!(whole.pages.iter().map(|p| (p.folder.as_str(), p.messages.len())).collect::<Vec<_>>(), [("M1", 2), ("M2", 1)]);
        assert_eq!((whole.changes.len(), whole.errors.join(", ").as_str(), whole.listing.map(|l| l.folders.len())), (1, "One, Two", Some(1)));
        // The progress of the piece that had one stays until another says more.
        assert_eq!(whole.progress.map(|p| p.total), Some(2));
    }

    /// The address of the account is in everything a request and a result are made of, and in
    /// none of what they print.
    #[test]
    fn debug_output_has_counts_only() {
        let (request, result) = (request(), result());
        for shown in [format!("{request:?}"), format!("{request:#?}"), format!("{result:?}"), format!("{result:#?}")] {
            assert!(!shown.contains('@'), "{shown}");
            for secret in ["example.org", "mail.", "Ada", "Bob", "Hello", "jmap/session", "E-", "M-", "may not delete"] {
                assert!(!shown.contains(secret), "{secret} in {shown}");
            }
        }
        assert_eq!(
            format!("{request:?}"),
            "JmapMailRequest { account: 3, bearer: true, window: 300, list_folders: true, scope: \"light+2\", mailboxes: 2, known: 3, changes: 3, sends: 1, background: true }"
        );
        assert_eq!(
            format!("{result:?}"),
            "JmapMailResult { account: 3, changes: 4, sent: 1, listed: Some(1), pages: 1, messages: 1, errors: 1, busy_for: Some(30), partial: true, whole: true, stopped: true, progress: true, auth_refused: true }"
        );
    }

    #[test]
    fn the_new_marks_of_the_server_are_written_only_when_they_are_set() {
        let plain = ServerMessage { id: "AAkA".into(), folder: "F1".into(), ..Default::default() };
        assert_eq!(round_trip(&plain), r#"{"id":"AAkA","folder":"F1","read":false,"flag":"none","importance":"normal"}"#);
        let marked = ServerMessage { answered: true, forwarded: true, ..plain };
        assert_eq!(
            round_trip(&marked),
            r#"{"id":"AAkA","folder":"F1","read":false,"answered":true,"forwarded":true,"flag":"none","importance":"normal"}"#
        );
        // One from before reads as neither answered nor forwarded.
        let old: ServerMessage = serde_json::from_str(r#"{"id":"AAkA","folder":"F1","read":true}"#).unwrap();
        assert!(old.read && !old.answered && !old.forwarded);
    }
}
