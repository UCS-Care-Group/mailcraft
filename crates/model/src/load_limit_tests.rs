// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The budget a mailbox file is read within: every list element, map entry and compound value at
//! any depth is charged as it is read, and a read past the budget stops with its reason.

use crate::Mailbox;
use crate::load_limit::{Budget, TOO_MANY_ITEMS, TOO_MUCH, read_json};

/// A one-message mailbox whose `field` is a list of `n` copies of `element`.
fn one_message_with(field: &str, element: &str, n: usize) -> String {
    let list = vec![element; n].join(",");
    format!(r#"{{"version":1,"messages":{{"1":{{"{field}":[{list}]}}}}}}"#)
}

#[test]
fn lists_inside_a_message_are_charged_element_by_element() {
    for (field, element) in
        [("attachments", "{}"), ("to", r#"{"email":""}"#), ("references", r#""""#), ("categories", r#""""#), ("headers", r#"["",""]"#)]
    {
        let json = one_message_with(field, element, 100_000);
        let budget = Budget::new(usize::MAX, 4 << 20);
        let e = read_json::<Mailbox>(json.as_bytes(), &budget).unwrap_err().to_string();
        assert!(e.contains(TOO_MUCH), "{field}: {e}");
        assert_eq!(budget.spent(), Some(TOO_MUCH), "{field}");
        // With room, it reads.
        let budget = Budget::new(usize::MAX, 1 << 30);
        assert!(read_json::<Mailbox>(json.as_bytes(), &budget).is_ok(), "{field}");
    }
}

#[test]
fn an_element_costs_at_least_what_it_takes_in_memory() {
    let json = one_message_with("attachments", "{}", 10_000);
    let budget = Budget::new(usize::MAX, u64::MAX);
    let mb = read_json::<Mailbox>(json.as_bytes(), &budget).unwrap();
    assert_eq!(mb.messages.values().next().unwrap().attachments.len(), 10_000);
    let charged = u64::MAX - budget.bytes_left();
    // Each one's slot, twice over for the room a growing list keeps.
    assert!(charged >= 10_000 * 2 * std::mem::size_of::<crate::Attachment>() as u64, "{charged}");
}

#[test]
fn items_are_still_counted() {
    let json = r#"{"messages":{"1":{},"2":{},"3":{}}}"#;
    let budget = Budget::new(2, u64::MAX);
    let e = read_json::<Mailbox>(json.as_bytes(), &budget).unwrap_err().to_string();
    assert!(e.contains(TOO_MANY_ITEMS), "{e}");
    assert_eq!(budget.spent(), Some(TOO_MANY_ITEMS));
}

#[test]
fn what_a_lenient_field_holds_is_charged_and_still_read_leniently() {
    // `synced` is read leniently: what isn't one is left out, and what is is charged.
    let ok = r#"{"messages":{"1":{"synced":"not flags"}}}"#;
    assert!(read_json::<Mailbox>(ok.as_bytes(), &Budget::new(usize::MAX, 1 << 30)).is_ok());
    let mut deep = String::from(r#"{"messages":{"1":{"synced":["#);
    deep.push_str(&vec!["[]"; 200_000].join(","));
    deep.push_str("]}}}");
    let budget = Budget::new(usize::MAX, 4 << 20);
    assert!(read_json::<Mailbox>(deep.as_bytes(), &budget).is_err());
    assert_eq!(budget.spent(), Some(TOO_MUCH));
}

#[test]
fn deep_nesting_is_refused_by_the_parser_on_a_small_thread() {
    let mut json = String::from(r#"{"accounts":[{"kind":"later","extra":"#);
    json.push_str(&"[".repeat(10_000));
    json.push_str(&"]".repeat(10_000));
    json.push_str("}]}");
    let read = std::thread::Builder::new()
        .stack_size(2 << 20)
        .spawn(move || read_json::<Mailbox>(json.as_bytes(), &Budget::new(usize::MAX, 1 << 30)).is_err())
        .unwrap()
        .join()
        .unwrap();
    assert!(read);
}

#[test]
fn a_budget_in_force_comes_back_after_a_read_within_another() {
    let outer = Budget::new(usize::MAX, 1 << 20);
    let inner = Budget::new(usize::MAX, 1 << 30);
    let json = one_message_with("references", r#""""#, 50_000);
    crate::load_limit::within(&outer, || {
        assert!(read_json::<Mailbox>(json.as_bytes(), &inner).is_ok());
    });
    assert!(outer.spent().is_none());
    // Nothing of the outer budget was taken by the inner read, and credit taken ahead goes back.
    assert_eq!(outer.bytes_left(), 1 << 20);
    assert!(inner.bytes_left() < 1 << 30);
}

#[test]
fn a_charge_refused_inside_a_lenient_field_refuses_the_read() {
    // `synced` is read leniently: a refusal while it is read must not leave it out of a mailbox
    // that then reads, to be saved without it.
    let message = crate::Message {
        subject: "Kept".into(),
        synced: Some(crate::sync::RemoteFlags { uid: 7, ..Default::default() }),
        server: Some(Default::default()),
        moving_out: Some(Default::default()),
        answers: Some(Default::default()),
        ..Default::default()
    };
    let json = format!(r#"{{"version":1,"messages":{{"1":{}}}}}"#, serde_json::to_string(&message).unwrap());
    let whole = Budget::new(usize::MAX, u64::MAX);
    read_json::<Mailbox>(json.as_bytes(), &whole).unwrap();
    let cost = u64::MAX - whole.bytes_left();
    let mut read = 0;
    for room in 0..=cost {
        let budget = Budget::new(usize::MAX, room);
        if let Ok(mb) = read_json::<Mailbox>(json.as_bytes(), &budget) {
            read += 1;
            let m = mb.messages.values().next().unwrap();
            assert!(
                m.synced.is_some() && m.server.is_some() && m.moving_out.is_some() && m.answers.is_some(),
                "{room} of {cost}: read without a field"
            );
        }
    }
    assert!(read >= 1);
}
