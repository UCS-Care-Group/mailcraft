// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A mailbox searched on its server (#1725): how the search is written, and the search against
//! the strict stand-in, with made-up mail.

use std::sync::atomic::AtomicBool;

use mailcraft_model::search::Query;
use mailcraft_model::sync::{EwsSearchRequest, GraphSearchResult};

use crate::find::{query_string, restriction};
use crate::standin::{AUTHORIZATION, Hostile, StandIn, block_on};
use crate::*;

const URL: &str = "http://127.0.0.1:1/EWS/Exchange.asmx";

fn client() -> Client {
    Client::new(URL, Secret::new(AUTHORIZATION)).unwrap()
}

fn written(query: &str) -> Option<String> {
    query_string(&Query::parse(query).for_server().terms)
}

fn restricted(query: &str) -> Option<String> {
    restriction(&Query::parse(query).for_server().terms).map(|element| element.write())
}

/// A mailbox with budget mail in the Inbox and in a folder of its own, and other mail.
fn mailbox() -> (StandIn, String, String) {
    let standin = StandIn::new();
    let (inbox, projects) = {
        let mut state = standin.state();
        let projects = state.add_folder("Projects");
        state.deliver("inbox", "Budget for the spring", true);
        state.deliver("inbox", "Lunch on Friday", false);
        state.deliver("inbox", "Budget review notes", false);
        state.deliver(&projects, "The budget of the old project", true);
        state.deliver(&projects, "Minutes", true);
        (state.folder_id("inbox").unwrap(), projects)
    };
    (standin, inbox, projects)
}

fn request(folders: &[&str], query: &str, next: &str) -> EwsSearchRequest {
    EwsSearchRequest {
        account: 4,
        email: "ada@example.org".into(),
        url: URL.into(),
        ticket: 9,
        folders: folders.iter().map(|id| (*id).to_owned()).collect(),
        query: query.into(),
        next: next.into(),
        ..Default::default()
    }
}

fn search(standin: &StandIn, request: &EwsSearchRequest) -> GraphSearchResult {
    let cancel = AtomicBool::new(false);
    block_on(search_messages(&client(), standin, request, &cancel, &mut |_| true)).unwrap()
}

fn subjects(result: &GraphSearchResult) -> Vec<&str> {
    result.messages.iter().map(|m| m.subject.as_str()).collect()
}

/// Nothing that writes anything was asked, and every request was `FindItem` alone.
fn only_searched(standin: &StandIn) {
    let state = standin.state();
    assert!(state.writes.is_empty(), "{:?}", state.writes);
    assert!(state.requests.iter().all(|asked| asked.operation == "FindItem"), "{:?}", state.requests);
}

#[test]
fn a_query_string_says_each_term_as_text_joined_by_and() {
    assert_eq!(written("budget").as_deref(), Some("budget"));
    assert_eq!(written("budget from:zoe subject:\"spring plan\"").as_deref(), Some("budget AND from:zoe AND subject:\"spring plan\""));
    assert_eq!(written("has:attachment importance:high").as_deref(), Some("hasattachment:true AND importance:high"));
    // An operator of the syntax, and a word with marks in it, are text in quotes.
    assert_eq!(written("OR").as_deref(), Some("\"OR\""));
    assert_eq!(written("body:q3-report.pdf").as_deref(), Some("body:\"q3-report.pdf\""));
    // What a server isn't asked leaves nothing to send.
    assert_eq!(written("is:unread"), None);
    assert_eq!(written("-budget"), None);
}

#[test]
fn a_word_never_carries_syntax_into_the_query_or_the_restriction() {
    let terms = vec![mailcraft_model::search::ServerTerm::Words { field: None, words: vec!["bud\"get\\ OR \u{1}x".into(), "\"".into()] }];
    assert_eq!(query_string(&terms).as_deref(), Some("\"budget OR x\""));
    let element = restriction(&terms).unwrap().write();
    assert!(element.contains("Value=\"budget OR x\"") && !element.contains('\\') && !element.contains('\u{1}'), "{element}");
    // A value is an attribute, escaped as one.
    let terms =
        vec![mailcraft_model::search::ServerTerm::Words { field: Some(mailcraft_model::search::ServerField::Subject), words: vec!["a<b>&c".into()] }];
    let element = restriction(&terms).unwrap().write();
    assert!(element.contains("a&lt;b&gt;&amp;c") && !element.contains("<b>"), "{element}");
}

#[test]
fn a_restriction_says_the_same_terms_and_none_for_what_it_cannot_say() {
    let one = restricted("subject:budget").unwrap();
    assert!(one.starts_with("<t:Contains ") && one.contains("FieldURI=\"item:Subject\"") && one.contains("Value=\"budget\""), "{one}");
    let anywhere = restricted("budget").unwrap();
    assert!(anywhere.starts_with("<t:Or>") && anywhere.contains("item:Body") && anywhere.contains("message:From"), "{anywhere}");
    let both = restricted("from:zoe has:attachment importance:low").unwrap();
    assert!(both.starts_with("<t:And>") && both.contains("item:HasAttachments") && both.contains("Value=\"Low\""), "{both}");
    assert_eq!(restricted("attachment:report"), None);
    assert_eq!(restricted("is:unread"), None);
}

#[test]
fn every_folder_named_is_searched_and_what_it_finds_comes_newest_first_with_its_folder() {
    let (standin, inbox, projects) = mailbox();
    let result = search(&standin, &request(&[&inbox, &projects], "budget", ""));
    assert!(result.errors.is_empty() && result.next.is_empty() && !result.offline, "{result:?}");
    assert_eq!((result.account, result.email.as_str(), result.ticket), (4, "ada@example.org", 9));
    assert_eq!(subjects(&result), ["The budget of the old project", "Budget review notes", "Budget for the spring"]);
    let folders: Vec<&str> = result.messages.iter().map(|m| m.server.folder.as_str()).collect();
    assert_eq!(folders, [projects.as_str(), inbox.as_str(), inbox.as_str()]);
    // What a list shows of each: the sender's name, the preview, whether it was read.
    let notes = &result.messages[1];
    assert_eq!((notes.from.name.as_str(), notes.preview.as_str(), notes.server.read), ("Zoe Park", "Hello about Budget review notes.", false));
    assert!(!notes.server.id.is_empty() && notes.received > 0 && !notes.message_id.is_empty());
    // One request a folder, each a query string, newest first.
    let state = standin.state();
    assert_eq!(state.requests.len(), 2);
    let body = &state.requests[0].body;
    assert!(body.contains("<m:QueryString>budget</m:QueryString>") && body.contains("Order=\"Descending\""), "{body}");
    assert!(body.contains("Traversal=\"Shallow\"") && !body.contains("m:Restriction"), "{body}");
    drop(state);
    only_searched(&standin);
}

#[test]
fn a_field_named_in_the_search_is_that_field_on_the_server() {
    let (standin, inbox, _) = mailbox();
    assert_eq!(subjects(&search(&standin, &request(&[&inbox], "subject:lunch", ""))), ["Lunch on Friday"]);
    assert_eq!(search(&standin, &request(&[&inbox], "to:zoe", "")).messages.len(), 0);
    assert_eq!(search(&standin, &request(&[&inbox], "from:zoe budget", "")).messages.len(), 2);
    assert_eq!(search(&standin, &request(&[&inbox], "has:attachment", "")).messages.len(), 0);
}

#[test]
fn a_search_goes_on_from_where_the_last_answer_ended() {
    let (standin, inbox, projects) = mailbox();
    {
        let mut state = standin.state();
        state.find_page = 1;
        for i in 0..60 {
            state.deliver("inbox", &format!("Budget line {i}"), true);
        }
    }
    let folders = [inbox.as_str(), projects.as_str()];
    // A page a request, forty requests an answer: the Inbox isn't through, and the answer says where it is.
    let first = search(&standin, &request(&folders, "budget", ""));
    assert_eq!(first.messages.len(), crate::search::MAX_ASKS);
    assert!(first.errors.is_empty() && first.next.ends_with(&inbox), "{first:?}");
    assert_eq!(standin.state().requests.len(), crate::search::MAX_ASKS);
    let second = search(&standin, &request(&folders, "budget", &first.next));
    assert!(second.next.is_empty() && second.errors.is_empty(), "{second:?}");
    // Every match once, the other folder's among them.
    let mut all: Vec<String> = [&first, &second].iter().flat_map(|r| r.messages.iter().map(|m| m.server.id.clone())).collect();
    assert_eq!(all.len(), 63);
    all.sort();
    all.dedup();
    assert_eq!(all.len(), 63);
    assert!(second.messages.iter().any(|m| m.subject == "The budget of the old project"));
    // A place the folders asked about don't have any more ends the search without a request.
    let before = standin.state().requests.len();
    let gone = search(&standin, &request(&[&projects], "budget", &first.next));
    assert!(gone.messages.is_empty() && gone.next.is_empty() && gone.errors.is_empty());
    assert_eq!(standin.state().requests.len(), before);
    // And what isn't a place at all begins at the start.
    assert_eq!(search(&standin, &request(&[&projects], "budget", "junk")).messages.len(), 1);
}

#[test]
fn a_full_page_ends_the_answer_and_the_rest_waits_for_the_next() {
    let (standin, inbox, projects) = mailbox();
    {
        let mut state = standin.state();
        for i in 0..70 {
            state.deliver("inbox", &format!("Budget line {i}"), true);
        }
    }
    let folders = [inbox.as_str(), projects.as_str()];
    let first = search(&standin, &request(&folders, "budget", ""));
    assert_eq!(first.messages.len(), crate::search::PAGE);
    assert_eq!(standin.state().requests.len(), 1, "a full page isn't followed by another folder");
    let second = search(&standin, &request(&folders, "budget", &first.next));
    assert_eq!(second.messages.len(), 23);
    assert!(second.next.is_empty());
}

#[test]
fn a_server_without_a_query_string_is_asked_with_a_restriction_for_the_rest_of_the_search() {
    let (standin, inbox, projects) = mailbox();
    {
        let mut state = standin.state();
        state.lacks_query_string = true;
        state.find_page = 1;
    }
    let folders = [inbox.as_str(), projects.as_str()];
    let found = search(&standin, &request(&folders, "budget", ""));
    assert!(found.errors.is_empty(), "{found:?}");
    assert_eq!(found.messages.len(), 3);
    {
        let state = standin.state();
        // The query string once, refused as the schema's fault; then restrictions alone.
        assert!(state.requests[0].body.contains("m:QueryString") && state.requests[0].code != "NoError");
        assert!(state.requests[1..].iter().all(|asked| asked.body.contains("m:Restriction") && !asked.body.contains("m:QueryString")));
    }
    // The next page knows how to ask without trying the query string again.
    standin.state().requests.clear();
    let from_the_second = search(&standin, &request(&[&inbox], "budget", &format!("1 r {inbox}")));
    assert_eq!(from_the_second.messages.len(), 1);
    assert!(standin.state().requests.iter().all(|asked| !asked.body.contains("m:QueryString")));
    // What a restriction can't say is said in words, not sent as something else.
    standin.state().requests.clear();
    let refused = search(&standin, &request(&folders, "attachment:report", ""));
    assert!(refused.messages.is_empty() && refused.errors.len() == 1, "{refused:?}");
    assert!(standin.state().requests.iter().all(|asked| !asked.body.contains("m:Restriction")));
    only_searched(&standin);
}

#[test]
fn a_server_without_the_preview_is_asked_again_without_it() {
    let (standin, inbox, _) = mailbox();
    standin.state().lacks_preview = true;
    let found = search(&standin, &request(&[&inbox], "budget", ""));
    assert!(found.errors.is_empty(), "{found:?}");
    assert_eq!(found.messages.len(), 2);
    assert!(found.messages.iter().all(|m| m.preview.is_empty()));
    let state = standin.state();
    assert_eq!(state.requests.len(), 2);
    assert!(state.requests[0].body.contains("item:Preview") && !state.requests[1].body.contains("item:Preview"));
    assert!(state.requests[1].body.contains("m:QueryString"), "still a query string");
}

#[test]
fn a_folder_that_is_gone_is_passed_over_and_the_others_are_still_asked() {
    let (standin, inbox, projects) = mailbox();
    let gone = "AAMkAGV4YW1wbGUtb3JnL2V3cy17Z29uZX0AAA=";
    let found = search(&standin, &request(&[gone, "bad\u{1}id", &projects, &inbox, &inbox], "budget", ""));
    assert!(found.errors.is_empty() && found.next.is_empty(), "{found:?}");
    assert_eq!(found.messages.len(), 3);
    // (An id that is none was not sent, and a folder named twice was asked once.)
    assert_eq!(standin.state().requests.len(), 3);
}

#[test]
fn what_stops_a_search_is_said_without_the_search_and_with_where_to_go_on() {
    let (standin, inbox, _) = mailbox();
    // A busy server that stays busy.
    standin.state().busy = (10, 1_500);
    let busy = search(&standin, &request(&[&inbox], "budget from:zoe", ""));
    assert_eq!(busy.busy_for, Some(2));
    assert!(busy.messages.is_empty() && !busy.next.is_empty());
    assert!(busy.errors.iter().all(|line| !line.contains("budget") && !line.contains("zoe") && !line.contains(&inbox)), "{busy:?}");
    // The owner's patience runs out while it is busy: nothing is said, nothing found.
    standin.state().busy = (10, 1_500);
    let cancel = AtomicBool::new(false);
    let request = request(&[&inbox], "budget", "");
    let called_off = block_on(search_messages(&client(), &standin, &request, &cancel, &mut |_| false)).unwrap();
    assert!(called_off.errors.is_empty() && called_off.messages.is_empty());
    // Called off before it began.
    standin.state().busy = (0, 0);
    let before = standin.state().requests.len();
    let cancel = AtomicBool::new(true);
    let nothing = block_on(search_messages(&client(), &standin, &request, &cancel, &mut |_| true)).unwrap();
    assert!(nothing.messages.is_empty() && nothing.errors.is_empty());
    assert_eq!(standin.state().requests.len(), before);
    // A password the server turns down.
    let wrong = Client::new(URL, Secret::new("Basic bm9ib2R5Om5vdGhpbmc=")).unwrap();
    let cancel = AtomicBool::new(false);
    let refused = block_on(search_messages(&wrong, &standin, &request, &cancel, &mut |_| true)).unwrap();
    assert_eq!(refused.errors, ["Couldn't search this mailbox: the server turned down the sign-in"]);
}

#[test]
fn nothing_to_ask_is_said_and_nothing_is_sent() {
    let (standin, inbox, _) = mailbox();
    let nothing = search(&standin, &request(&[&inbox], "is:unread", ""));
    assert_eq!(nothing.errors.len(), 1);
    let none = search(&standin, &request(&[], "budget", ""));
    assert!(none.errors.is_empty() && none.messages.is_empty() && none.next.is_empty());
    assert!(standin.state().requests.is_empty());
}

#[test]
fn hostile_answers_to_a_search_are_passed_over_or_said_and_never_trusted() {
    let (standin, inbox, projects) = mailbox();
    // Items whose ids are no ids: none is taken, and the search ends.
    standin.state().hostile = Some(Hostile::BadItems);
    let found = search(&standin, &request(&[&inbox, &projects], "budget", ""));
    assert!(found.messages.iter().all(|m| m.server.id.len() <= 1024), "{found:?}");
    // Every other kind of answer that isn't one: an error at the most, never a panic, and
    // nothing taken from it.
    for how in [Hostile::BadId, Hostile::Dtd, Hostile::CutShort, Hostile::NotXml, Hostile::Deep, Hostile::Fault, Hostile::TooFew] {
        standin.state().hostile = Some(how);
        let found = search(&standin, &request(&[&inbox, &projects], "budget", ""));
        assert!(found.messages.len() <= 3, "{how:?}");
        assert!(found.messages.iter().all(|m| m.server.id.len() <= 1024 && [&inbox, &projects].contains(&&m.server.folder)), "{how:?}");
    }
}

#[test]
fn an_answer_that_lists_nothing_or_lies_about_its_place_ends_the_folder() {
    use crate::find::{FindItem, Search};
    use crate::items::Shape;
    let find = FindItem::new("folder", Search::Query("budget".into()), 10, 50, Shape::Lean);
    let answer = |root: &str| {
        let body = format!(
            "<?xml version=\"1.0\"?><s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\"><s:Body>\
             <m:FindItemResponse xmlns:m=\"http://schemas.microsoft.com/exchange/services/2006/messages\" \
             xmlns:t=\"http://schemas.microsoft.com/exchange/services/2006/types\"><m:ResponseMessages>\
             <m:FindItemResponseMessage ResponseClass=\"Success\"><m:ResponseCode>NoError</m:ResponseCode>{root}\
             </m:FindItemResponseMessage></m:ResponseMessages></m:FindItemResponse></s:Body></s:Envelope>"
        );
        find.answer(&Reply::parse(body.as_bytes()).unwrap())
    };
    // Says there is more, lists nothing: there is nothing after it, or the search would never end.
    let empty = answer("<m:RootFolder IncludesLastItemInRange=\"false\" IndexedPagingOffset=\"0\"><t:Items/></m:RootFolder>").unwrap();
    assert!(empty.last && empty.messages.is_empty());
    // A next page that isn't further on is not followed back.
    let item = "<t:Message><t:ItemId Id=\"one\" ChangeKey=\"k\"/><t:Subject>Budget</t:Subject></t:Message>";
    let back = answer(&format!("<m:RootFolder IncludesLastItemInRange=\"false\" IndexedPagingOffset=\"3\"><t:Items>{item}</t:Items></m:RootFolder>"))
        .unwrap();
    assert_eq!((back.last, back.next_offset, back.messages.len()), (false, 11, 1));
    // An item with no id is passed over, and an answer with no folder is none.
    let unnamed =
        answer("<m:RootFolder IncludesLastItemInRange=\"true\"><t:Items><t:Message><t:Subject>x</t:Subject></t:Message></t:Items></m:RootFolder>")
            .unwrap();
    assert!(unnamed.messages.is_empty() && unnamed.last);
    assert!(answer("").is_err());
}

#[test]
fn an_exchange_2010_is_searched_in_its_own_version_without_the_preview() {
    let (standin, inbox, _) = mailbox();
    standin.state().exchange2010 = true;
    let found = search(&standin, &request(&[&inbox], "budget", ""));
    assert!(found.errors.is_empty(), "{found:?}");
    assert_eq!(found.messages.len(), 2);
    let state = standin.state();
    let last = state.requests.last().unwrap();
    assert!(last.body.contains("m:QueryString") && !last.body.contains("item:Preview"), "{}", last.body);
}
