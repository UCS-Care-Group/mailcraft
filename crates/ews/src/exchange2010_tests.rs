// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An organisation's own Exchange 2010 (#1349), against the strict stand-in behaving as one: the
//! version stepped down once, the item properties its schema hasn't left out, the recipients asked
//! with `GetItem`, and changes of a message named with the item's change key.

use std::cell::Cell;
use std::future::Future;
use std::sync::atomic::AtomicBool;

use mailcraft_model::FlagStatus;
use mailcraft_model::sync::{DraftUp, EwsMailRequest, EwsMailResult, FlagUp, MessageChange, MessageOutcome};

use crate::standin::{AUTHORIZATION, StandIn, block_on};
use crate::*;

const URL: &str = "https://mail.example.org/EWS/Exchange.asmx";

fn client() -> Client {
    Client::new(URL, Secret::new(AUTHORIZATION)).unwrap()
}

fn exchange2010() -> StandIn {
    let standin = StandIn::new();
    standin.state().exchange2010 = true;
    standin
}

fn request() -> EwsMailRequest {
    EwsMailRequest { email: "ada@example.org".into(), url: URL.into(), list_folders: true, ..Default::default() }
}

fn run_as(client: &Client, standin: &StandIn, request: &EwsMailRequest) -> Vec<EwsMailResult> {
    let mut pieces = Vec::new();
    let cancel = AtomicBool::new(false);
    block_on(sync_mail(client, standin, request, &cancel, &mut |_| true, &mut |piece| pieces.push(piece))).unwrap();
    pieces
}

fn run(standin: &StandIn, request: &EwsMailRequest) -> EwsMailResult {
    let mut all = EwsMailResult::default();
    for piece in run_as(&client(), standin, request) {
        all.absorb(piece);
    }
    all
}

/// The codes the stand-in answered the requests of `operation` with, in order.
fn codes(standin: &StandIn, operation: &str) -> Vec<String> {
    standin.state().requests.iter().filter(|r| r.operation == operation).map(|r| r.code.clone()).collect()
}

#[test]
fn the_account_test_steps_down_to_exchange_2010_once_and_stays_there() {
    let standin = exchange2010();
    let client = client();
    assert_eq!(client.version(), Version::Exchange2013);
    let inbox = block_on(client.check(&standin)).unwrap().unwrap();
    assert_eq!(inbox.display_name, "Inbox");
    assert_eq!(client.version(), Version::Exchange2010Sp2);
    block_on(client.check(&standin)).unwrap().unwrap();
    assert_eq!(standin.state().versions, ["Exchange2013", "Exchange2010_SP2", "Exchange2010_SP2"], "asked once in the newer version, not again");
    // A newer server is never stepped down.
    let newer = StandIn::new();
    let client = self::client();
    block_on(client.check(&newer)).unwrap().unwrap();
    assert_eq!((client.version(), newer.state().versions.len()), (Version::Exchange2013, 1));
}

/// Answers every request with the fault of a version the server doesn't know.
struct NoVersion {
    asked: Cell<u32>,
}

impl HttpExchange for NoVersion {
    fn exchange(&self, _: &HttpRequest) -> impl Future<Output = Result<HttpResponse, TransportError>> {
        self.asked.set(self.asked.get() + 1);
        let body = format!(
            "<?xml version=\"1.0\"?><s:Envelope xmlns:s=\"{}\"><s:Body><s:Fault><faultcode>a:ErrorInvalidServerVersion</faultcode><faultstring>No.</faultstring>\
             <detail><e:ResponseCode xmlns:e=\"{}\">ErrorInvalidServerVersion</e:ResponseCode></detail></s:Fault></s:Body></s:Envelope>",
            soap::SOAP,
            soap::ERRORS
        );
        std::future::ready(Ok(HttpResponse::new(500, body.into_bytes())))
    }
}

#[test]
fn the_version_steps_down_one_step_and_no_further() {
    let server = NoVersion { asked: Cell::new(0) };
    let client = client();
    assert_eq!(block_on(client.check(&server)).unwrap(), Err(Error::Server(ResponseCode::InvalidServerVersion)));
    assert_eq!(server.asked.get(), 2, "the version asked for, then the fallback, and nothing more");
    assert_eq!(client.version(), Version::Exchange2010Sp2);
    assert_eq!(block_on(client.check(&server)).unwrap(), Err(Error::Server(ResponseCode::InvalidServerVersion)));
    assert_eq!(server.asked.get(), 3, "once stepped down, a refusal is the answer");
}

#[test]
fn mail_from_exchange_2010_comes_down_with_its_recipients_and_categories() {
    let standin = exchange2010();
    let id = standin.state().deliver("inbox", "Kickoff", false);
    {
        let mut s = standin.state();
        s.deliver("inbox", "Budget", true);
        let item = s.items.iter_mut().find(|i| i.id == id).unwrap();
        item.cc = vec![("Kit".into(), "kit@example.org".into())];
        item.categories = vec!["Blue".into()];
    }
    let pieces = run_as(&client(), &standin, &request());
    let messages: Vec<_> = pieces.iter().flat_map(|p| &p.pages).flat_map(|page| &page.messages).collect();
    assert_eq!(messages.iter().map(|m| m.subject.as_str()).collect::<Vec<_>>(), ["Kickoff", "Budget"]);
    let kickoff = messages[0];
    assert_eq!(kickoff.to.iter().map(|a| a.email.as_str()).collect::<Vec<_>>(), ["ada@example.org"]);
    assert_eq!(kickoff.cc.iter().map(|a| a.email.as_str()).collect::<Vec<_>>(), ["kit@example.org"]);
    assert_eq!(kickoff.from.email, "zoe@example.org");
    assert_eq!(kickoff.server.categories, ["Blue"]);
    assert!(!kickoff.server.change_key.is_empty(), "the change key is kept from the listing");
    assert_eq!(messages[1].to.len(), 1);
    assert!(pieces.last().unwrap().whole, "{:?}", pieces.last().unwrap().errors);
    // The server's version was learnt before the folders were read: no listing was refused.
    assert!(codes(&standin, "SyncFolderItems").iter().all(|c| c == "NoError"), "{:?}", codes(&standin, "SyncFolderItems"));
    let state = standin.state();
    let syncs: Vec<_> = state.requests.iter().filter(|r| r.operation == "SyncFolderItems").collect();
    assert!(syncs.iter().all(|r| !r.body.contains("item:Preview") && !r.body.contains("message:ToRecipients")));
    assert!(state.requests.iter().any(|r| r.operation == "GetItem" && r.body.contains("message:CcRecipients") && r.code == "NoError"));
}

#[test]
fn a_server_refusing_the_preview_and_then_the_recipients_steps_down_to_what_it_takes() {
    // A server that takes the newer version but has Exchange 2010's schema: the schema fault and the
    // refusal of the recipients each step the listing down, for the rest of the run.
    let standin = exchange2010();
    standin.state().takes_any_version = true;
    standin.state().deliver("inbox", "Kickoff", false);
    standin.state().deliver("sentitems", "Sent one", true);
    let result = run(&standin, &request());
    assert!(result.whole, "{:?}", result.errors);
    let messages: Vec<_> = result.pages.iter().flat_map(|p| &p.messages).collect();
    assert_eq!(messages.len(), 2);
    assert!(messages.iter().all(|m| m.to.len() == 1 && m.preview.is_empty()));
    let codes = codes(&standin, "SyncFolderItems");
    assert_eq!(codes.get(..2), Some(&["ErrorSchemaValidation".to_owned(), "ErrorInvalidPropertyForOperation".to_owned()][..]));
    assert!(codes.len() > 3 && codes[2..].iter().all(|c| c == "NoError"), "every folder after is read in the shape it takes: {codes:?}");
}

#[test]
fn a_message_on_exchange_2010_is_marked_read_with_its_change_key() {
    let standin = exchange2010();
    let first = standin.state().deliver("inbox", "Kickoff", false);
    let second = standin.state().deliver("inbox", "Budget", false);
    let change =
        |message, id: &str| MessageChange::Update { message, id: id.to_owned(), read: Some(true), flag: None, categories: None, importance: None };
    let result = run(&standin, &EwsMailRequest { messages: vec![change(1, &first), change(2, &second)], ..request() });
    for at in 0..2 {
        assert!(matches!(result.messages[at].outcome, MessageOutcome::Done { .. }), "{:?}", result.messages[at].outcome);
    }
    let state = standin.state();
    assert!(state.item(&first).unwrap().read && state.item(&second).unwrap().read);
    drop(state);
    // Refused once without the key; asked for it, and every change after that is named with it.
    assert_eq!(codes(&standin, "UpdateItem"), ["ErrorChangeKeyRequiredForWriteOperations", "NoError", "NoError"]);
}

#[test]
fn a_server_that_still_refuses_with_the_key_has_the_change_refused_once() {
    let standin = exchange2010();
    let id = standin.state().deliver("inbox", "Kickoff", false);
    // The item's key reads as none: the server is asked for it, and the change is refused, not looped.
    standin.state().items[0].change_key = String::new();
    let change = MessageChange::Update { message: 1, id: id.clone(), read: Some(true), flag: None, categories: None, importance: None };
    let result = run(&standin, &EwsMailRequest { messages: vec![change], ..request() });
    assert!(matches!(result.messages[0].outcome, MessageOutcome::Refused { .. }), "{:?}", result.messages[0].outcome);
    assert_eq!(codes(&standin, "UpdateItem").len(), 2);
    assert!(!standin.state().item(&id).unwrap().read);
}

#[test]
fn a_draft_on_exchange_2010_known_without_a_key_is_changed_with_it() {
    let standin = exchange2010();
    let id = standin.state().deliver("drafts", "Draft", true);
    let up = DraftUp { message: 5, id: id.clone(), subject: Some("Draft, again".into()), ..Default::default() };
    let result = run(&standin, &EwsMailRequest { drafts: vec![up], ..request() });
    let done = &result.drafts[0];
    assert!(done.fields && done.refused.is_none() && !done.conflict, "{done:?}");
    assert_eq!(standin.state().item(&id).unwrap().subject, "Draft, again");
    assert_eq!(codes(&standin, "UpdateItem"), ["ErrorChangeKeyRequiredForWriteOperations", "NoError"]);
}

#[test]
fn a_flag_stays_here_and_the_read_mark_goes_up_on_exchange_2010() {
    let flagged = || Some(FlagUp { status: FlagStatus::Flagged, ..Default::default() });
    // Known to be Exchange 2010 by its version: the flag isn't sent at all.
    let standin = exchange2010();
    let id = standin.state().deliver("inbox", "Kickoff", false);
    let change = MessageChange::Update { message: 1, id: id.clone(), read: Some(true), flag: flagged(), categories: None, importance: None };
    let result = run(&standin, &EwsMailRequest { messages: vec![change], ..request() });
    assert!(matches!(result.messages[0].outcome, MessageOutcome::Done { .. }), "{:?}", result.messages[0].outcome);
    assert!(standin.state().item(&id).unwrap().read);
    assert!(standin.state().requests.iter().filter(|r| r.operation == "UpdateItem").all(|r| !r.body.contains("item:Flag")));
    // A server that takes the newer version but faults on the flag: the flag is dropped and the rest goes.
    let standin = exchange2010();
    standin.state().takes_any_version = true;
    let id = standin.state().deliver("inbox", "Kickoff", false);
    let change = MessageChange::Update { message: 1, id: id.clone(), read: Some(true), flag: flagged(), categories: None, importance: None };
    let result = run(&standin, &EwsMailRequest { messages: vec![change], ..request() });
    assert!(matches!(result.messages[0].outcome, MessageOutcome::Done { .. }), "{:?}", result.messages[0].outcome);
    assert!(standin.state().item(&id).unwrap().read);
    assert_eq!(codes(&standin, "UpdateItem"), ["ErrorChangeKeyRequiredForWriteOperations", "ErrorSchemaValidation", "NoError"]);
}
