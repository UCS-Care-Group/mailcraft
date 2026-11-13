// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The identity and submission examples of RFC 8621 (§6.4, §7.5.1), each call's arguments read into the
//! method's types and written back. Edits: line breaks of the page layout inside strings joined with a
//! space, and the `...` standing for more recipients removed.

mod support;

use mailcraft_jmap_types::{EmailSubmission, EmailSubmissionSetRequest, GetRequest, GetResponse, Identity, SetResponse, set_error};
use serde_json::json;
use support::round_trip;

#[test]
fn identity_get() {
    let get: GetRequest = round_trip(r#"{ "accountId": "acme" }"#);
    assert_eq!(get.account_id, "acme");
    let got: GetResponse<Identity> = round_trip(
        r#"{
          "accountId": "acme",
          "state": "99401312ae-11-333",
          "list": [
            {
              "id": "XD-3301-222-11_22AAz",
              "name": "Joe Bloggs",
              "email": "joe@example.com",
              "replyTo": null,
              "bcc": [{ "name": null, "email": "joe+archive@example.com" }],
              "textSignature": "-- \nJoe Bloggs\nMaster of Email",
              "htmlSignature": "<div><b>Joe Bloggs</b></div> <div>Master of Email</div>",
              "mayDelete": false
            },
            {
              "id": "XD-9911312-11_22AAz",
              "name": "Joe B",
              "email": "*@example.com",
              "replyTo": null,
              "bcc": null,
              "textSignature": "",
              "htmlSignature": "",
              "mayDelete": true
            }
          ],
          "notFound": []
        }"#,
    );
    let joe = &got.list[0];
    assert_eq!(joe.email.as_deref(), Some("joe@example.com"));
    assert_eq!(joe.reply_to, None);
    let bcc = &joe.bcc.as_ref().unwrap()[0];
    assert_eq!((bcc.name.as_deref(), bcc.email.as_str()), (None, "joe+archive@example.com"));
    assert_eq!(joe.text_signature.as_deref(), Some("-- \nJoe Bloggs\nMaster of Email"));
    assert_eq!(got.list[1].may_delete, Some(true));
}

#[test]
fn email_submission_set_with_an_update_on_success() {
    let set: EmailSubmissionSetRequest = round_trip(
        r##"{
          "accountId": "ue411d190",
          "create": {
            "k1490": {
              "identityId": "I64588216",
              "emailId": "M7f6ed5bcfd7e2604d1753f6c",
              "envelope": {
                "mailFrom": { "email": "john@example.com", "parameters": null },
                "rcptTo": [{ "email": "jane@example.com", "parameters": null }]
              }
            }
          },
          "onSuccessUpdateEmail": {
            "#k1490": {
              "mailboxIds/7cb4e8ee-df87-4757-b9c4-2ea1ca41b38e": null,
              "mailboxIds/73dbcb4b-bffc-48bd-8c2a-a2e91ca672f6": true,
              "keywords/$draft": null
            }
          }
        }"##,
    );
    let submission = &set.set.create.as_ref().unwrap()["k1490"];
    assert_eq!(submission.identity_id.as_deref(), Some("I64588216"));
    let envelope = submission.envelope.as_ref().unwrap();
    assert_eq!(envelope.mail_from.email, "john@example.com");
    assert_eq!(envelope.rcpt_to[0].parameters, None);
    // The patch keeps its nulls: they take the email out of the drafts mailbox and clear `$draft`.
    let patch = &set.on_success_update_email.as_ref().unwrap()["#k1490"];
    assert_eq!(
        serde_json::to_value(patch).unwrap(),
        json!({
            "mailboxIds/7cb4e8ee-df87-4757-b9c4-2ea1ca41b38e": null,
            "mailboxIds/73dbcb4b-bffc-48bd-8c2a-a2e91ca672f6": true,
            "keywords/$draft": null
        })
    );
    let done: SetResponse<EmailSubmission> = round_trip(
        r#"{
          "accountId": "ue411d190",
          "oldState": "012421s6-8nrq-4ps4-n0p4-9330r951ns21",
          "newState": "355421f6-8aed-4cf4-a0c4-7377e951af36",
          "created": { "k1490": { "id": "ES-3bab7f9a-623e-4acf-99a5-2e67facb02a0" } }
        }"#,
    );
    assert_eq!(done.created.unwrap()["k1490"].id.as_deref(), Some("ES-3bab7f9a-623e-4acf-99a5-2e67facb02a0"));
}

#[test]
fn email_submission_refused() {
    let refused: SetResponse<EmailSubmission> = round_trip(
        r#"{
          "accountId": "ue411d190",
          "oldState": "012421s6-8nrq-4ps4-n0p4-9330r951ns21",
          "newState": "012421s6-8nrq-4ps4-n0p4-9330r951ns21",
          "notCreated": {
            "k1490": {
              "type": "forbiddenToSend",
              "description": "Verzeihung, wegen verdaechtiger Aktivitaeten Ihres Benutzerkontos haben wir den Versand von Nachrichten gesperrt. Bitte wenden Sie sich fuer Hilfe an unser Support Team."
            }
          }
        }"#,
    );
    let error = &refused.not_created.unwrap()["k1490"];
    assert_eq!(error.kind, set_error::FORBIDDEN_TO_SEND);
    assert!(error.description.as_deref().unwrap().starts_with("Verzeihung"));
}

/// RFC 8621 §7: the properties the server sets on a submission as it goes, as §7 lists them (the RFC has no
/// example of a whole one).
#[test]
fn email_submission_as_the_server_reports_it() {
    let got: EmailSubmission = round_trip(
        r#"{
          "id": "ES1",
          "identityId": "I1",
          "emailId": "M1",
          "threadId": "T1",
          "envelope": {
            "mailFrom": { "email": "john@example.com", "parameters": { "RET": "HDRS", "ENVID": null } },
            "rcptTo": [{ "email": "jane@example.com" }]
          },
          "sendAt": "2018-07-10T01:05:08Z",
          "undoStatus": "final",
          "deliveryStatus": {
            "jane@example.com": { "smtpReply": "250 2.0.0 OK", "delivered": "yes", "displayed": "unknown" }
          },
          "dsnBlobIds": [],
          "mdnBlobIds": []
        }"#,
    );
    assert_eq!(got.undo_status.as_deref(), Some("final"));
    let status = &got.delivery_status.as_ref().unwrap()["jane@example.com"];
    assert_eq!((status.smtp_reply.as_str(), status.delivered.as_str()), ("250 2.0.0 OK", "yes"));
    let parameters = got.envelope.unwrap().mail_from.parameters.unwrap();
    assert_eq!(parameters["RET"].as_deref(), Some("HDRS"));
    assert_eq!(parameters["ENVID"], None);
}
