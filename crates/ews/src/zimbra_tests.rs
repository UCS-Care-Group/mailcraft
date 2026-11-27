// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Zimbra's EWS, against the strict stand-in behaving as it does: its `SyncFolderItems` leaves an
//! item's `ItemId` out unless it is asked for the ids alone, so a folder's changes were a page that
//! can't be taken in and nothing of the mailbox came down. The listing steps down to the ids, and
//! what a list shows of each message is asked with `GetItem`.

use std::sync::atomic::AtomicBool;

use mailcraft_model::sync::{EwsMailRequest, EwsMailResult};

use crate::standin::{AUTHORIZATION, Hostile, StandIn, block_on};
use crate::*;

const URL: &str = "https://mail.example.org/EWS/Exchange.asmx";

fn client() -> Client {
    Client::new(URL, Secret::new(AUTHORIZATION)).unwrap()
}

fn request() -> EwsMailRequest {
    EwsMailRequest { email: "ada@example.org".into(), url: URL.into(), list_folders: true, ..Default::default() }
}

fn run(standin: &StandIn) -> EwsMailResult {
    let cancel = AtomicBool::new(false);
    let mut all = EwsMailResult::default();
    block_on(sync_mail(&client(), standin, &request(), &cancel, &mut |_| true, &mut |piece| all.absorb(piece))).unwrap();
    all
}

#[test]
fn mail_from_a_server_that_leaves_the_ids_out_of_its_changes_comes_down_whole() {
    let standin = StandIn::new();
    {
        let mut s = standin.state();
        s.zimbra_ids = true;
        let id = s.deliver("inbox", "Kickoff", false);
        s.deliver("inbox", "Budget", true);
        let item = s.items.iter_mut().find(|i| i.id == id).unwrap();
        item.cc = vec![("Kit".into(), "kit@example.org".into())];
        item.categories = vec!["Blue".into()];
    }
    let result = run(&standin);
    assert!(result.whole, "{:?}", result.errors);
    let messages: Vec<_> = result.pages.iter().flat_map(|p| &p.messages).collect();
    assert_eq!(messages.iter().map(|m| m.subject.as_str()).collect::<Vec<_>>(), ["Kickoff", "Budget"]);
    let kickoff = messages[0];
    assert!(!kickoff.server.id.is_empty() && !kickoff.cut);
    assert_eq!(kickoff.from.email, "zoe@example.org");
    assert_eq!(kickoff.to.iter().map(|a| a.email.as_str()).collect::<Vec<_>>(), ["ada@example.org"]);
    assert_eq!(kickoff.cc.iter().map(|a| a.email.as_str()).collect::<Vec<_>>(), ["kit@example.org"]);
    assert_eq!(kickoff.server.categories, ["Blue"]);
    assert!(messages[1].server.read && kickoff.received != 0);
    let state = standin.state();
    // Once stepped down, every listing asks for the ids alone, and GetItem for the rest.
    let syncs: Vec<_> = state.requests.iter().filter(|r| r.operation == "SyncFolderItems").collect();
    assert!(syncs.iter().skip(1).all(|r| r.body.contains(">IdOnly<") && !r.body.contains("AdditionalProperties")), "the ids alone after the first");
    assert!(state.requests.iter().any(|r| r.operation == "GetItem" && r.body.contains("item:Subject") && r.code == "NoError"));
}

#[test]
fn a_message_gone_since_it_was_listed_is_left_out_and_the_page_taken_in() {
    let standin = StandIn::new();
    let gone = {
        let mut s = standin.state();
        s.zimbra_ids = true;
        s.deliver("inbox", "Kept", false);
        let gone = s.deliver("inbox", "Gone", false);
        s.gone_from_get_item = vec![gone.clone()];
        gone
    };
    let result = run(&standin);
    assert!(result.whole, "{:?}", result.errors);
    let messages: Vec<_> = result.pages.iter().flat_map(|p| &p.messages).collect();
    assert_eq!(messages.iter().map(|m| m.subject.as_str()).collect::<Vec<_>>(), ["Kept"]);
    assert!(messages.iter().all(|m| m.server.id != gone));
}

/// A message kept as its id alone would read as one with nothing set, over what is kept of it
/// (its read state, flag, categories): a page the server won't say what a list shows of is not
/// taken in, and is read again.
#[test]
fn a_page_whose_messages_getitem_refuses_is_held_back() {
    let standin = StandIn::new();
    {
        let mut s = standin.state();
        s.zimbra_ids = true;
        s.deliver("inbox", "Kickoff", true);
        s.refuses_get_item = true;
    }
    let result = run(&standin);
    assert!(!result.whole);
    assert!(result.pages.iter().all(|p| p.messages.is_empty()), "nothing of the page is taken in");
    standin.state().refuses_get_item = false;
    let again = run(&standin);
    assert!(again.whole, "{:?}", again.errors);
    let messages: Vec<_> = again.pages.iter().flat_map(|p| &p.messages).collect();
    assert_eq!(messages.len(), 1);
    assert!(messages[0].server.read && messages[0].subject == "Kickoff");
}

/// A folder the ids alone don't make readable either (here, items whose ids are too long to
/// keep) is blocked, and the folders after it are listed in the shape they were: not item by
/// item, which would leave the preview and the flag out.
#[test]
fn a_folder_the_ids_alone_dont_make_readable_leaves_the_others_as_they_were() {
    let standin = StandIn::new();
    let bad = {
        let mut s = standin.state();
        let bad = s.add_folder("Box");
        s.deliver(&bad, "In the box", false);
        s.deliver("inbox", "Kickoff", false);
        s.hostile = Some(Hostile::BadItems);
        s.bad_items_in = bad.clone();
        bad
    };
    let result = run(&standin);
    assert_eq!(result.blocked, vec![bad]);
    assert!(result.pages.iter().flat_map(|p| &p.messages).any(|m| m.subject == "Kickoff"));
    let state = standin.state();
    assert!(!state.requests.iter().any(|r| r.operation == "GetItem" && r.body.contains("item:IsDraft")), "no folder was listed item by item");
}

#[test]
fn the_list_fields_are_asked_without_the_body_and_read_by_the_id_asked() {
    let op = GetItemListed::new(&["A1".to_owned(), "\u{1}bad".to_owned(), "A2".to_owned()]);
    let request = op.element().write();
    assert!(request.contains(">IdOnly<") && request.contains("item:Subject") && request.contains("message:From"));
    assert!(
        !request.contains("item:Preview") && !request.contains("item:Flag") && !request.contains("item:Body"),
        "only Exchange 2010's schema, no body"
    );
    assert!(request.contains("item:IsDraft"), "a draft is listed as one");
    assert_eq!(request.matches("<t:ItemId ").count(), 2, "an id with a control character in it isn't asked");
}
