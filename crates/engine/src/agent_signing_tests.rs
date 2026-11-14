// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Security review of #1036: an agent may turn Sign on for the message being written, but
//! turning it off is the owner's to say, every time; and the question about sending says whether
//! the message goes Digitally signed, and with what.

use serde_json::json;

use super::{Access, Class, Verdict, check, classify, describe};
use crate::Session;

const FP: &str = "abababababababababababababababababababababababababababababababab";

/// An IMAP account with a key; whether it always signs.
fn writing(always: bool) -> Session {
    let mut s = Session::empty();
    let a = s.run("account.add", &json!({"email": "ada@mail.example", "incoming": {"host": "imap.mail.example"}})).unwrap()["id"].as_u64().unwrap();
    s.run("account.setSigning", &json!({"account": a, "key": {"format": "smime", "fingerprint": FP, "label": "Ada Example"}, "always": always}))
        .unwrap();
    s.run("mail.new", &json!({"to": "maya@lumen.example", "subject": "Rota", "account": a})).unwrap();
    s
}

#[test]
fn an_agent_turns_sign_on_freely_and_off_only_with_the_owner() {
    let s = writing(true);
    // Signed now (Always): off, or the other way round, is asked about every time.
    for params in [json!({"value": false}), json!({})] {
        assert_eq!(classify(&s, "compose.sign", &params), Access::Asked(Class::Send), "{params}");
        match check(&s, "compose.sign", &params, &Class::ALL) {
            Verdict::Ask(Class::Send, what) => assert!(what.contains("without a Digital signature") && what.contains("Rota"), "{what}"),
            other => panic!("{params}: {other:?}"),
        }
    }
    // On again changes nothing that needs anybody.
    assert_eq!(check(&s, "compose.sign", &json!({"value": true}), &[]), Verdict::Run);

    let s = writing(false);
    // Unsigned now: on, the other way round (on) and off (no change) need nobody.
    for params in [json!({"value": true}), json!({}), json!({"value": false})] {
        assert_eq!(check(&s, "compose.sign", &params, &[]), Verdict::Run, "{params}");
    }
}

#[test]
fn the_question_about_sending_says_how_it_is_signed() {
    let mut s = writing(true);
    let d = describe(&s, "compose.send", &json!({}));
    assert!(d.contains("Digitally signed with the S/MIME certificate") && d.contains("Ada Example") && d.contains("…ABABABAB"), "{d}");
    s.run("compose.sign", &json!({"value": false})).unwrap();
    let d = describe(&s, "compose.send", &json!({}));
    assert!(d.contains("without a Digital signature"), "{d}");
    // To be signed, and no key: Send refuses it, and the question says so.
    let a = s.compose.as_ref().unwrap().account;
    s.run("compose.sign", &json!({"value": true})).unwrap();
    s.mailbox.account_mut(a).unwrap().signing.key = None;
    let d = describe(&s, "compose.send", &json!({}));
    assert!(d.contains("can't be yet"), "{d}");
}
