// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #1074: whether a flag was changed here since the server last said. A Microsoft server keeps a
//! follow-up's due date and "complete" itself, so both are compared, as they always were; a JMAP
//! server keeps `$flagged` alone, so only that bit is.

use mailcraft_model::sync::ServerMessage;
use mailcraft_model::time::Timestamp;
use mailcraft_model::{AccountKind, FlagStatus, Folder, FolderKind, Mailbox, Message, ServerFolder};

use super::*;

const DUE: Timestamp = 1_800_000_000;

/// An account of `kind` with one message in its Inbox: flagged `here`, and `last` as its server last said.
fn mailbox(kind: AccountKind, here: (FlagStatus, Option<Timestamp>), last: (FlagStatus, Option<Timestamp>)) -> (Mailbox, Id, Id) {
    let mut mb = Mailbox::with_local_account("avery@mail.example", "Avery");
    let account = mb.accounts[0].id;
    mb.accounts[0].kind = kind;
    let folder = mb.alloc_id();
    let server = Some(ServerFolder { id: "F".into(), ..Default::default() });
    mb.folders.push(Folder { id: folder, account, name: "Inbox".into(), kind: FolderKind::Inbox, server, ..Default::default() });
    let server = ServerMessage { id: "E1".into(), folder: "F".into(), flag: last.0, flag_due: last.1, ..Default::default() };
    let m = Message { folder, flag: here.0, flag_due: here.1, server: Some(server), subject: "Follow up".into(), ..Default::default() };
    let id = mb.add_message(m).unwrap();
    (mb, account, id)
}

/// The message's flag once a page says the server has it as `now`.
fn after(
    kind: AccountKind,
    here: (FlagStatus, Option<Timestamp>),
    last: (FlagStatus, Option<Timestamp>),
    now: (FlagStatus, Option<Timestamp>),
) -> (FlagStatus, Option<Timestamp>) {
    let (mut mb, account, id) = mailbox(kind, here, last);
    let server = ServerMessage { id: "E1".into(), folder: "F".into(), flag: now.0, flag_due: now.1, ..Default::default() };
    let page = MessagePage { folder: "F".into(), messages: vec![GraphMessage { server, ..Default::default() }], ..Default::default() };
    take_page(&mut mb, &mut ServerIndex::default(), account, &page, &mut Down::default());
    let m = mb.message(id).unwrap();
    assert_eq!(m.server.as_ref().map(|server| (server.flag, server.flag_due)), Some(now), "what the server last said is what it says now");
    (m.flag, m.flag_due)
}

#[test]
fn a_microsoft_accounts_flag_is_compared_by_status_and_due_date_as_before() {
    use FlagStatus::{Complete, Flagged, None as Clear};
    let kind = AccountKind::Microsoft;
    // As the server last said, due date and all: it follows the server, either way.
    assert_eq!(after(kind, (Flagged, Some(DUE)), (Flagged, Some(DUE)), (Clear, None)), (Clear, None));
    assert_eq!(after(kind, (Complete, Some(DUE)), (Complete, Some(DUE)), (Flagged, Some(DUE + 1))), (Flagged, Some(DUE + 1)));
    // A due date given here, or marked complete here, since: changed here, so both stay, whatever
    // the server says now. That goes up to a server that keeps it.
    assert_eq!(after(kind, (Flagged, Some(DUE)), (Flagged, None), (Clear, None)), (Flagged, Some(DUE)));
    assert_eq!(after(kind, (Complete, Some(DUE)), (Flagged, Some(DUE)), (Flagged, Some(DUE))), (Complete, Some(DUE)));
    assert_eq!(after(kind, (Complete, None), (Clear, None), (Flagged, None)), (Complete, None));
}

#[test]
fn a_jmap_accounts_flag_is_compared_by_the_flagged_bit_alone() {
    use FlagStatus::{Complete, Flagged, None as Clear};
    let kind = AccountKind::Jmap;
    // The bit as the server last said, and another there: the server's state, without a due date.
    assert_eq!(after(kind, (Flagged, Some(DUE)), (Flagged, None), (Clear, None)), (Clear, None));
    assert_eq!(after(kind, (Complete, Some(DUE)), (Clear, None), (Flagged, None)), (Flagged, None));
    assert_eq!(after(kind, (Clear, None), (Clear, None), (Flagged, None)), (Flagged, None));
    // The bit as the server last said and says still: what only this computer keeps stays.
    assert_eq!(after(kind, (Flagged, Some(DUE)), (Flagged, None), (Flagged, None)), (Flagged, Some(DUE)));
    assert_eq!(after(kind, (Complete, Some(DUE)), (Clear, None), (Clear, None)), (Complete, Some(DUE)));
    // The bit changed here: it stays, whatever the server says (the same, or what it said before).
    assert_eq!(after(kind, (Flagged, Some(DUE)), (Clear, None), (Clear, None)), (Flagged, Some(DUE)));
    assert_eq!(after(kind, (Flagged, Some(DUE)), (Clear, None), (Flagged, None)), (Flagged, Some(DUE)));
    assert_eq!(after(kind, (Complete, Some(DUE)), (Flagged, None), (Flagged, None)), (Complete, Some(DUE)));
    assert_eq!(after(kind, (Complete, Some(DUE)), (Flagged, None), (Clear, None)), (Complete, Some(DUE)));
}
