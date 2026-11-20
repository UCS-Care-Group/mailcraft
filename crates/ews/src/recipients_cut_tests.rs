// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The recipients an Exchange server is asked for with `GetItem` (#1349, #1151), when they don't
//! come: a message whose recipients weren't answered is listed as cut, so that a rule that reads
//! who it was sent to waits for it to be here whole instead of taking it for mail sent to nobody
//! (#566).

use std::sync::atomic::AtomicBool;

use mailcraft_model::sync::{EwsMailRequest, GraphMessage};

use crate::standin::{AUTHORIZATION, StandIn, block_on, response};
use crate::standin_mail::ok_message;
use crate::*;

const URL: &str = "https://mail.example.org/EWS/Exchange.asmx";

fn exchange2010() -> StandIn {
    let standin = StandIn::new();
    standin.state().exchange2010 = true;
    standin
}

/// The messages one run lists, in order.
fn listed(standin: &StandIn) -> Vec<GraphMessage> {
    let client = Client::new(URL, Secret::new(AUTHORIZATION)).unwrap();
    let request = EwsMailRequest { email: "ada@example.org".into(), url: URL.into(), list_folders: true, ..Default::default() };
    let mut pieces = Vec::new();
    let cancel = AtomicBool::new(false);
    block_on(sync_mail(&client, standin, &request, &cancel, &mut |_| true, &mut |piece| pieces.push(piece))).unwrap();
    pieces.into_iter().flat_map(|p| p.pages).flat_map(|page| page.messages).collect()
}

#[test]
fn recipients_the_server_wont_give_leave_the_message_cut() {
    let standin = exchange2010();
    standin.state().deliver("inbox", "Kickoff", false);
    standin.state().refuses_get_item = true;
    let messages = listed(&standin);
    assert_eq!(messages.len(), 1);
    assert!(messages[0].to.is_empty() && messages[0].cut, "{:?}", messages[0]);
    // Answered, they make a message that isn't cut.
    let standin = exchange2010();
    standin.state().deliver("inbox", "Kickoff", false);
    let messages = listed(&standin);
    assert!(messages[0].to.len() == 1 && !messages[0].cut, "{:?}", messages[0]);
}

#[test]
fn a_message_the_server_leaves_out_of_its_answer_is_cut_and_the_others_are_not() {
    let standin = exchange2010();
    let gone = standin.state().deliver("inbox", "Kickoff", false);
    standin.state().deliver("inbox", "Budget", false);
    standin.state().gone_from_get_item = vec![gone];
    let messages = listed(&standin);
    let cut: Vec<(&str, bool)> = messages.iter().map(|m| (m.subject.as_str(), m.cut)).collect();
    assert_eq!(cut, [("Kickoff", true), ("Budget", false)]);
}

#[test]
fn recipients_answered_with_some_left_out_are_cut() {
    let read = |to: &str| {
        let item = format!("<t:Message><t:ItemId Id=\"ID1\" ChangeKey=\"CK\"/><t:ToRecipients>{to}</t:ToRecipients></t:Message>");
        let body = response("GetItem", &ok_message("GetItem", &format!("<m:Items>{item}</m:Items>")));
        let reply = Reply::parse(body.as_bytes()).unwrap();
        GetItemRecipients::new(&["ID1".to_owned()]).answer(&reply).unwrap().remove(0)
    };
    let one = |n: usize| format!("<t:Mailbox><t:EmailAddress>p{n}@example.org</t:EmailAddress></t:Mailbox>");
    let whole = read(&one(0));
    assert!(!whole.cut && whole.to.len() == 1);
    assert!(read(&(0..201).map(one).collect::<String>()).cut, "past what is kept of a list");
    let nobody = read(&format!("{}<t:Mailbox></t:Mailbox>", one(0)));
    assert!(nobody.cut && nobody.to.len() == 1, "a recipient with no address");
}

/// A message whose recipients the server answers with more than are kept of a list: it is listed
/// as cut, as the answer says, so that a rule that reads who it was sent to waits (#566).
#[test]
fn a_message_answered_with_more_recipients_than_are_kept_is_listed_cut() {
    let standin = exchange2010();
    let id = standin.state().deliver("inbox", "All staff", false);
    standin.state().deliver("inbox", "Budget", false);
    {
        let mut s = standin.state();
        let item = s.items.iter_mut().find(|i| i.id == id).unwrap();
        item.to = (0..201).map(|n| (format!("P{n}"), format!("p{n}@example.org"))).collect();
    }
    let messages = listed(&standin);
    let cut: Vec<(&str, bool)> = messages.iter().map(|m| (m.subject.as_str(), m.cut)).collect();
    assert_eq!(cut, [("All staff", true), ("Budget", false)]);
}

/// Exchange 2013 to 2019 leave the recipients out of the listing (#1151), so they are asked with
/// `GetItem` too: refused, the messages are listed as cut, as on Exchange 2010. A message that came
/// with its recipients is not cut, even beside one that came without (#566).
#[test]
fn recipients_left_out_of_the_listing_and_refused_leave_the_message_cut_and_those_listed_are_not() {
    let standin = StandIn::new();
    {
        let mut s = standin.state();
        s.omits_recipients = true;
        s.refuses_get_item = true;
        s.deliver("inbox", "Kickoff", false);
    }
    let messages = listed(&standin);
    assert_eq!(messages.len(), 1);
    assert!(messages[0].to.is_empty() && messages[0].cut, "{:?}", messages[0]);
    let standin = StandIn::new();
    {
        let mut s = standin.state();
        s.refuses_get_item = true;
        s.deliver("inbox", "Kickoff", false);
        let notes = s.deliver("inbox", "Notes", false);
        s.items.iter_mut().find(|i| i.id == notes).unwrap().to.clear();
    }
    let messages = listed(&standin);
    let cut: Vec<(&str, usize, bool)> = messages.iter().map(|m| (m.subject.as_str(), m.to.len(), m.cut)).collect();
    assert_eq!(cut, [("Kickoff", 1, false), ("Notes", 0, true)]);
}
