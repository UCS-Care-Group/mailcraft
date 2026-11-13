// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The examples of RFC 8621 (JMAP mail), each call's arguments read into the method's types and written
//! back.
//!
//! Edits to the RFC's text: the `...` standing for more entries or more text removed (a string cut short
//! by one keeps the text before it), and line breaks of the page layout inside strings joined with a
//! space. Each fixture is one invocation's arguments; other edits are said where they are made.

mod support;

use mailcraft_jmap_types::{
    ChangesRequest, Email, EmailGetRequest, EmailQueryChangesRequest, EmailQueryRequest, Filter, GetRequest, GetResponse, Mailbox,
    MailboxChangesResponse, MailboxFilterCondition, MailboxSetRequest, QueryChangesResponse, QueryResponse, SetRequest, SetResponse, Thread,
    set_error,
};
use serde_json::json;
use support::round_trip;

/// §2.6: all the mailboxes. The RFC asks with `"ids": null`, which is the default (RFC 8620 §3.5) and is
/// written back as nothing.
#[test]
fn mailbox_get_all() {
    let get: GetRequest = round_trip(r#"{ "accountId": "u33084183", "ids": null }"#);
    assert_eq!(get.ids, None);
    assert_eq!(serde_json::to_value(&get).unwrap(), json!({"accountId": "u33084183"}));
}

#[test]
fn mailbox_get_response() {
    let got: GetResponse<Mailbox> = round_trip(
        r#"{
          "accountId": "u33084183",
          "state": "78540",
          "list": [{
            "id": "MB23cfa8094c0f41e6",
            "name": "Inbox",
            "parentId": null,
            "role": "inbox",
            "sortOrder": 10,
            "totalEmails": 16307,
            "unreadEmails": 13905,
            "totalThreads": 5833,
            "unreadThreads": 5128,
            "myRights": {
              "mayAddItems": true,
              "mayRename": false,
              "maySubmit": true,
              "mayDelete": false,
              "maySetKeywords": true,
              "mayRemoveItems": true,
              "mayCreateChild": true,
              "maySetSeen": true,
              "mayReadItems": true
            },
            "isSubscribed": true
          }, {
            "id": "MB674cc24095db49ce",
            "name": "Important mail"
          }],
          "notFound": []
        }"#,
    );
    let inbox = &got.list[0];
    assert_eq!(inbox.role.as_deref(), Some("inbox"));
    assert_eq!(inbox.parent_id, None);
    assert_eq!((inbox.total_emails, inbox.unread_emails), (Some(16307), Some(13905)));
    let rights = inbox.my_rights.as_ref().unwrap();
    assert!(rights.may_submit && !rights.may_rename && rights.may_read_items);
    assert_eq!(got.list[1].name.as_deref(), Some("Important mail"));
    assert_eq!(got.list[1].role, None);
}

#[test]
fn mailbox_changes_and_partial_get() {
    let changes: ChangesRequest = round_trip(r#"{ "accountId": "u33084183", "sinceState": "78540" }"#);
    assert_eq!(changes.max_changes, None);
    let changed: MailboxChangesResponse = round_trip(
        r#"{
          "accountId": "u33084183",
          "oldState": "78541",
          "newState": "78542",
          "hasMoreChanges": false,
          "updatedProperties": [ "totalEmails", "unreadEmails", "totalThreads", "unreadThreads" ],
          "created": [],
          "updated": ["MB23cfa8094c0f41e6"],
          "destroyed": []
        }"#,
    );
    assert_eq!(changed.changes.updated, ["MB23cfa8094c0f41e6"]);
    assert_eq!(changed.updated_properties.unwrap().len(), 4);
    let got: GetResponse<Mailbox> = round_trip(
        r#"{
          "accountId": "u33084183",
          "state": "78542",
          "list": [{
            "id": "MB23cfa8094c0f41e6",
            "totalEmails": 16307,
            "unreadEmails": 13903,
            "totalThreads": 5833,
            "unreadThreads": 5127
          }],
          "notFound": []
        }"#,
    );
    assert_eq!(got.list[0].name, None);
    assert_eq!(got.list[0].unread_threads, Some(5127));
}

#[test]
fn mailbox_set_rename_and_destroy() {
    let set: MailboxSetRequest = round_trip(
        r#"{
          "accountId": "u33084183",
          "ifInState": "78542",
          "update": { "MB674cc24095db49ce": { "name": "Maybe important mail" } },
          "destroy": [ "MB23cfa8094c0f41e6" ]
        }"#,
    );
    assert_eq!(set.set.destroy.as_deref(), Some(&["MB23cfa8094c0f41e6".to_owned()][..]));
    assert_eq!(set.on_destroy_remove_emails, None);
    let done: SetResponse<Mailbox> = round_trip(
        r#"{
          "accountId": "u33084183",
          "oldState": "78542",
          "newState": "78549",
          "updated": { "MB674cc24095db49ce": null },
          "notDestroyed": { "MB23cfa8094c0f41e6": { "type": "forbidden" } }
        }"#,
    );
    // An update the server changed nothing more of is a null, and stays one.
    assert_eq!(serde_json::to_value(&done).unwrap()["updated"], json!({"MB674cc24095db49ce": null}));
    assert_eq!(done.not_destroyed.unwrap()["MB23cfa8094c0f41e6"].kind, set_error::FORBIDDEN);
}

/// §3.1.1, as printed.
#[test]
fn thread_get() {
    let get: GetRequest = round_trip(r#"{ "accountId": "acme", "ids": ["f123u4", "f41u44"] }"#);
    assert_eq!(get.ids.unwrap().len(), 2);
    let got: GetResponse<Thread> = round_trip(
        r#"{
          "accountId": "acme",
          "state": "f6a7e214",
          "list": [
            { "id": "f123u4", "emailIds": [ "eaa623", "f782cbb"] },
            { "id": "f41u44", "emailIds": [ "82cf7bb" ] }
          ],
          "notFound": []
        }"#,
    );
    assert_eq!(got.list[0].email_ids.as_deref(), Some(&["eaa623".to_owned(), "f782cbb".to_owned()][..]));
}

/// §2.3: a filter on `parentId: null` asks for the top level, which is not the same as no filter on the
/// parent at all; both are kept apart.
#[test]
fn a_mailbox_filter_tells_a_null_parent_from_none() {
    let top: MailboxFilterCondition = round_trip(r#"{"parentId": null}"#);
    assert_eq!(top.parent_id, Some(None));
    assert_eq!(serde_json::to_value(&top).unwrap(), json!({"parentId": null}));
    let any: MailboxFilterCondition = round_trip(r#"{"hasAnyRole": true}"#);
    assert_eq!(any.parent_id, None);
    assert_eq!(serde_json::to_value(&any).unwrap(), json!({"hasAnyRole": true}));
}

/// §4.2.1. Edits: `accountId`, which §4.2 (RFC 8620 §5.1) requires, put in the request; the `...` that
/// cut the two body values short removed.
#[test]
fn email_get_with_header_and_body_properties() {
    let get: EmailGetRequest = round_trip(
        r##"{
          "accountId": "abc",
          "ids": [ "f123u456", "f123u457" ],
          "properties": [ "threadId", "mailboxIds", "from", "subject", "receivedAt", "header:List-POST:asURLs", "htmlBody", "bodyValues" ],
          "bodyProperties": [ "partId", "blobId", "size", "type" ],
          "fetchHTMLBodyValues": true,
          "maxBodyValueBytes": 256
        }"##,
    );
    assert_eq!(get.fetch_html_body_values, Some(true));
    assert_eq!(get.max_body_value_bytes, Some(256));
    assert_eq!(get.get.properties.unwrap().len(), 8);
    let got: GetResponse<Email> = round_trip(
        r#"{
          "accountId": "abc",
          "state": "41234123231",
          "list": [
            {
              "id": "f123u457",
              "threadId": "ef1314a",
              "mailboxIds": { "f123": true },
              "from": [{ "name": "Joe Bloggs", "email": "joe@example.com" }],
              "subject": "Dinner on Thursday?",
              "receivedAt": "2013-10-13T14:12:00Z",
              "header:List-POST:asURLs": [ "mailto:partytime@lists.example.com" ],
              "htmlBody": [{
                "partId": "1",
                "blobId": "B841623871",
                "size": 283331,
                "type": "text/html"
              }, {
                "partId": "2",
                "blobId": "B319437193",
                "size": 10343,
                "type": "text/plain"
              }],
              "bodyValues": {
                "1": { "isEncodingProblem": false, "isTruncated": true, "value": "<html><body><p>Hello " },
                "2": { "isEncodingProblem": false, "isTruncated": false, "value": "-- Sent by your friendly mailing list " }
              }
            }
          ],
          "notFound": [ "f123u456" ]
        }"#,
    );
    assert_eq!(got.not_found, ["f123u456"]);
    let email = &got.list[0];
    assert_eq!(email.subject.as_deref(), Some("Dinner on Thursday?"));
    let from = &email.from.as_ref().unwrap()[0];
    assert_eq!((from.name.as_deref(), from.email.as_str()), (Some("Joe Bloggs"), "joe@example.com"));
    assert_eq!(email.mailbox_ids.as_ref().unwrap().get("f123"), Some(&true));
    // A header field in a form the client named is kept as JSON under its own property name.
    assert_eq!(email.extra["header:List-POST:asURLs"], json!(["mailto:partytime@lists.example.com"]));
    let html = &email.html_body.as_ref().unwrap()[0];
    assert_eq!((html.part_id.as_deref(), html.kind.as_deref(), html.size), (Some("1"), Some("text/html"), Some(283331)));
    let first = &email.body_values.as_ref().unwrap()["1"];
    assert_eq!((first.is_truncated, first.is_encoding_problem), (Some(true), Some(false)));
}

/// §4.10: the first request of a client's first look at the Inbox, and the answers. Edits: the `...` of
/// the lists removed, and the preview's text cut where it was.
#[test]
fn email_query_then_threads_then_emails() {
    let query: EmailQueryRequest = round_trip(
        r#"{
          "accountId": "ue150411c",
          "filter": { "inMailbox": "fb666a55" },
          "sort": [{ "isAscending": false, "property": "receivedAt" }],
          "collapseThreads": true,
          "position": 0,
          "limit": 30,
          "calculateTotal": true
        }"#,
    );
    assert_eq!(query.collapse_threads, Some(true));
    let Some(Filter::Condition(condition)) = &query.query.filter else { panic!("one condition") };
    assert_eq!(condition.in_mailbox.as_deref(), Some("fb666a55"));
    assert_eq!(query.query.sort.as_ref().unwrap()[0].is_ascending, Some(false));
    let found: QueryResponse = round_trip(
        r#"{
          "accountId": "ue150411c",
          "queryState": "09aa9a075588-780599:0",
          "canCalculateChanges": true,
          "position": 0,
          "total": 115,
          "ids": [ "Ma783e5cdf5f2deffbc97930a", "M9bd17497e2a99cb345fc1d0a" ]
        }"#,
    );
    assert_eq!(found.total, Some(115));
    let threads: GetResponse<Email> = round_trip(
        r#"{
          "accountId": "ue150411c",
          "state": "780599",
          "list": [
            { "id": "Ma783e5cdf5f2deffbc97930a", "threadId": "T36703c2cfe9bd5ed" },
            { "id": "M9bd17497e2a99cb345fc1d0a", "threadId": "T0a22ad76e9c097a1" }
          ],
          "notFound": []
        }"#,
    );
    assert_eq!(threads.list[1].thread_id.as_deref(), Some("T0a22ad76e9c097a1"));
    let shown: GetResponse<Email> = round_trip(
        r#"{
          "accountId": "ue150411c",
          "state": "780599",
          "list": [{
            "id": "Ma783e5cdf5f2deffbc97930a",
            "threadId": "T36703c2cfe9bd5ed",
            "mailboxIds": { "fb666a55": true },
            "keywords": { "$seen": true, "$flagged": true },
            "hasAttachment": true,
            "from": [{ "email": "jdoe@example.com", "name": "Jane Doe" }],
            "subject": "The Big Reveal",
            "receivedAt": "2018-06-27T00:20:35Z",
            "size": 175047,
            "preview": "As you may be aware, we are required to prepare a presentation where we wow a panel of 5 random members of the public, on or before 30 June each year.  We have drafted"
          }],
          "notFound": []
        }"#,
    );
    let email = &shown.list[0];
    assert_eq!(email.keywords.as_ref().unwrap().get("$flagged"), Some(&true));
    assert_eq!((email.has_attachment, email.size), (Some(true), Some(175047)));
}

/// §4.10: marking unread, then the changes since.
#[test]
fn email_set_keyword_then_changes_and_query_changes() {
    let unread: SetRequest<Email> =
        round_trip(r#"{ "accountId": "ue150411c", "update": { "Ma783e5cdf5f2deffbc97930a": { "keywords/$seen": null } } }"#);
    assert_eq!(unread.update.unwrap()["Ma783e5cdf5f2deffbc97930a"]["keywords/$seen"], serde_json::Value::Null);
    let done: SetResponse<Email> =
        round_trip(r#"{ "accountId": "ue150411c", "oldState": "780605", "newState": "780606", "updated": { "Ma783e5cdf5f2deffbc97930a": null } }"#);
    assert_eq!(done.updated.unwrap()["Ma783e5cdf5f2deffbc97930a"], None);
    let since: EmailQueryChangesRequest = round_trip(
        r#"{
          "accountId": "ue150411c",
          "filter": { "inMailbox": "fb666a55" },
          "sort": [{ "property": "receivedAt", "isAscending": false }],
          "collapseThreads": true,
          "sinceQueryState": "09aa9a075588-780599:0",
          "upToId": "Mc2781d5e856a908d8a35a564",
          "maxChanges": 25,
          "calculateTotal": true
        }"#,
    );
    assert_eq!(since.query_changes.up_to_id.as_deref(), Some("Mc2781d5e856a908d8a35a564"));
    let changed: QueryChangesResponse = round_trip(
        r#"{
          "accountId": "ue150411c",
          "oldQueryState": "09aa9a075588-780599:0",
          "newQueryState": "e35e9facf117-780615:0",
          "added": [{ "id": "Me8de6c9f6de198239b982ea2", "index": 0 }],
          "removed": [ "M9bd17497e2a99cb345fc1d0a" ],
          "total": 115
        }"#,
    );
    assert_eq!(changed.added[0].index, 0);
}

/// §4.10: a draft created with its body structure and values, then replaced by an HTML one. Edits: the
/// values' wrapped text joined, the `...` of the responses removed.
#[test]
fn email_set_creates_drafts() {
    let plain: SetRequest<Email> = round_trip(
        r#"{
          "accountId": "ue150411c",
          "create": {
            "k192": {
              "mailboxIds": { "2ea1ca41b38e": true },
              "keywords": { "$seen": true, "$draft": true },
              "from": [{ "name": "Joe Bloggs", "email": "joe@example.com" }],
              "subject": "World domination",
              "receivedAt": "2018-07-10T01:03:11Z",
              "sentAt": "2018-07-10T11:03:11+10:00",
              "bodyStructure": { "type": "text/plain", "partId": "bd48", "header:Content-Language": "en" },
              "bodyValues": {
                "bd48": { "value": "I have the most brilliant plan.  Let me tell you all about it.  What we do is, we", "isTruncated": false }
              }
            }
          }
        }"#,
    );
    let draft = &plain.create.unwrap()["k192"];
    assert_eq!(draft.sent_at.as_deref(), Some("2018-07-10T11:03:11+10:00"));
    let body = draft.body_structure.as_ref().unwrap();
    assert_eq!(body.extra["header:Content-Language"], "en");
    // Written back, a draft names only what it was given: no id, no `isEncodingProblem`.
    let written = serde_json::to_value(draft).unwrap();
    assert!(written.get("id").is_none());
    assert!(written["bodyValues"]["bd48"].get("isEncodingProblem").is_none());
    let created: SetResponse<Email> = round_trip(
        r#"{
          "accountId": "ue150411c",
          "oldState": "780823",
          "newState": "780839",
          "created": {
            "k192": {
              "id": "Mf40b5f831efa7233b9eb1c7f",
              "blobId": "Gf40b5f831efa7233b9eb1c7f8f97d84eeeee64f7",
              "threadId": "Td957e72e89f516dc",
              "size": 359
            }
          }
        }"#,
    );
    assert_eq!(created.created.unwrap()["k192"].blob_id.as_deref(), Some("Gf40b5f831efa7233b9eb1c7f8f97d84eeeee64f7"));
    let html: SetRequest<Email> = round_trip(
        r#"{
          "accountId": "ue150411c",
          "create": {
            "k1546": {
              "mailboxIds": { "2ea1ca41b38e": true },
              "keywords": { "$seen": true, "$draft": true },
              "from": [{ "name": "Joe Bloggs", "email": "joe@example.com" }],
              "to": [{ "name": "John", "email": "john@example.com" }],
              "subject": "World domination",
              "receivedAt": "2018-07-10T01:05:08Z",
              "sentAt": "2018-07-10T11:05:08+10:00",
              "bodyStructure": {
                "type": "multipart/alternative",
                "subParts": [
                  { "partId": "a49d", "type": "text/html", "header:Content-Language": "en" },
                  { "partId": "bd48", "type": "text/plain", "header:Content-Language": "en" }
                ]
              },
              "bodyValues": {
                "bd48": { "value": "I have the most brilliant plan.  Let me tell you all about it.  What we do is, we", "isTruncated": false },
                "a49d": {
                  "value": "<!DOCTYPE html><html><head><title></title> <style type=\"text/css\">div{font-size:16px}</style></head> <body><div>I have the most <b>brilliant</b> plan.  Let me tell you all about it.  What we do is, we</div></body> </html>",
                  "isTruncated": false
                }
              }
            }
          },
          "destroy": [ "Mf40b5f831efa7233b9eb1c7f" ]
        }"#,
    );
    let parts = html.create.unwrap()["k1546"].body_structure.clone().unwrap().sub_parts.unwrap();
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0].kind.as_deref(), Some("text/html"));
}

/// §4.10: the implicit Email/set of a draft moved to another account, which destroys the original.
#[test]
fn email_set_response_of_a_copys_destroy() {
    let destroyed: SetResponse<Email> =
        round_trip(r#"{ "accountId": "ue150411c", "oldState": "780842", "newState": "780871", "destroyed": [ "Md45b47b4877521042cec0938" ] }"#);
    assert_eq!(destroyed.destroyed.unwrap().len(), 1);
}
