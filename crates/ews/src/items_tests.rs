// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The reader of `SyncFolderItems`' answer, over answers made by hand: which changes make a page
//! and which make it no page (#252, E6). A page is whole or it is an error; what an item lacks is
//! read as nothing said.

use crate::standin::response;
use crate::standin_mail::ok_message;
use crate::*;
use mailcraft_model::FlagStatus;

fn page(changes: &str, max: u32) -> Result<ItemsPage, Error> {
    let inner =
        format!("<m:SyncState>S-NEW</m:SyncState><m:IncludesLastItemInRange>true</m:IncludesLastItemInRange><m:Changes>{changes}</m:Changes>");
    let body = response("SyncFolderItems", &ok_message("SyncFolderItems", &inner));
    let reply = Reply::parse(body.as_bytes()).unwrap();
    SyncFolderItems::new("FOLDER", "S-OLD", max, Shape::Full).answer(&reply)
}

fn bare(id: &str) -> String {
    format!("<t:Message><t:ItemId Id=\"{id}\" ChangeKey=\"CK\"/></t:Message>")
}

#[test]
fn more_changes_than_were_asked_for_are_taken_in_when_every_one_is_readable() {
    let changes: String = (0..50).map(|n| format!("<t:Create>{}</t:Create>", bare(&format!("ID{n}")))).collect();
    let got = page(&changes, 5).unwrap();
    assert_eq!((got.messages.len(), got.state.as_str(), got.last), (50, "S-NEW", true));
}

#[test]
fn an_item_that_lacks_what_is_not_essential_is_read_with_nothing_said() {
    // Only an id: no read state, importance, categories, preview, sender, times or size.
    let got = page(&format!("<t:Create>{}</t:Create><t:Update>{}</t:Update>", bare("ID1"), bare("ID2")), 10).unwrap();
    assert_eq!(got.messages.len(), 2);
    let one = &got.messages[0];
    assert_eq!((one.server.id.as_str(), one.server.read, one.server.categories.len(), one.subject.as_str()), ("ID1", false, 0, ""));
    assert!(one.preview.is_empty() && one.to.is_empty() && one.received == 0);
    // An id carrying markup or a space is no id: that's a server's own template leaking, and a
    // page whose item carries one is not read (#1784, measured on Kerio naming a whole folder
    // `<id>` and failing it every run). Characters a real id does carry — base64-ish and the
    // escaped few — are opaque and stay fine.
    assert_eq!(page(&format!("<t:Create>{}</t:Create>", bare("a b&lt;c&gt;/+=-_")), 10).unwrap_err(), Error::Unreadable);
    let odd = page(&format!("<t:Create>{}</t:Create>", bare("AAMk+Tk/Zm9=-_1")), 10).unwrap();
    assert_eq!(odd.messages[0].server.id, "AAMk+Tk/Zm9=-_1");
    // A read-state change that doesn't say which state carries nothing to lose: passed over.
    let none = page("<t:ReadFlagChange><t:ItemId Id=\"ID1\" ChangeKey=\"CK\"/></t:ReadFlagChange>", 10).unwrap();
    assert!(none.read.is_empty() && none.messages.is_empty() && none.read_unsaid == 1);
    // A kind of change this program doesn't know is passed over as well.
    let unknown = page("<t:Frobnicate><t:ItemId Id=\"ID1\"/></t:Frobnicate>", 10).unwrap();
    assert!(unknown.messages.is_empty() && unknown.removed.is_empty() && unknown.read.is_empty());
}

#[test]
fn a_change_with_no_usable_id_or_item_makes_the_page_no_page() {
    let long = "x".repeat(2_000);
    let bad: Vec<(&str, String)> = vec![
        ("a delete with no id", "<t:Delete><t:ItemId ChangeKey=\"CK\"/></t:Delete>".into()),
        ("a delete with an empty id", "<t:Delete><t:ItemId Id=\"\"/></t:Delete>".into()),
        ("a delete with a long id", format!("<t:Delete><t:ItemId Id=\"{long}\"/></t:Delete>")),
        ("a delete with a control character in the id", "<t:Delete><t:ItemId Id=\"A&#9;B\"/></t:Delete>".into()),
        ("a delete with no item", "<t:Delete/>".into()),
        ("a read-state change with a long id", format!("<t:ReadFlagChange><t:ItemId Id=\"{long}\"/><t:IsRead>true</t:IsRead></t:ReadFlagChange>")),
        ("a read-state change with no id", "<t:ReadFlagChange><t:IsRead>true</t:IsRead></t:ReadFlagChange>".into()),
        ("a create with nothing in it", "<t:Create/>".into()),
        ("a create with an item that has no id", "<t:Create><t:Message><t:Subject>Hello</t:Subject></t:Message></t:Create>".into()),
        ("an update with a long id", format!("<t:Update>{}</t:Update>", bare(&long))),
        ("an update with an empty id", format!("<t:Update>{}</t:Update>", bare(""))),
    ];
    for (what, change) in bad {
        // Alone, and after changes that are fine: either way nothing of the page is handed on.
        assert_eq!(page(&change, 10).unwrap_err(), Error::Unreadable, "{what}");
        let after = format!("<t:Create>{}</t:Create>{change}", bare("ID1"));
        assert_eq!(page(&after, 10).unwrap_err(), Error::Unreadable, "{what}, after a good one");
    }
    // A delete with a good id is a removal.
    let good = page("<t:Delete><t:ItemId Id=\"ID9\" ChangeKey=\"CK\"/></t:Delete>", 10).unwrap();
    assert_eq!(good.removed, ["ID9"]);
}

#[test]
fn a_page_larger_than_the_model_takes_in_is_no_page() {
    let changes: String =
        (0..mailcraft_model::sync::MAX_EWS_PAGE_MESSAGES + 1).map(|n| format!("<t:Create>{}</t:Create>", bare(&format!("ID{n}")))).collect();
    assert_eq!(page(&changes, 512).unwrap_err(), Error::Unreadable);
}

/// The read state named as `PR_IS_READ`, read however the server quotes the tag — hex with
/// or without the prefix and in either case, or plain decimal — and answered only in words
/// that mean yes or no. Anything else is no answer: silence, held, never an invented unread
/// (#1784, the review).
#[test]
fn the_read_state_tag_is_read_whatever_way_the_server_quotes_it() {
    for (tag, value, want) in [
        ("0E67", "true", Some(true)),
        ("0xe67", "False", Some(false)),
        ("3687", "1", Some(true)),
        ("0x0E67", "0", Some(false)),
        ("3687", "perhaps", None),
        ("1002", "1", None),
    ] {
        let xml = format!(
            "<t:Create><t:Message><t:ItemId Id=\"I1\" ChangeKey=\"C\"/><t:ExtendedProperty><t:ExtendedFieldURI PropertyTag=\"{tag}\" PropertyType=\"Boolean\"/><t:Value>{value}</t:Value></t:ExtendedProperty></t:Message></t:Create>"
        );
        let got = page(&xml, 10).unwrap();
        let m = &got.messages[0].server;
        assert_eq!((m.read, m.read_said), (want.unwrap_or(false), want.is_some()), "tag {tag}={value}");
    }
}

/// The flag as `PR_FLAG_STATUS`: on Kerio the only flag there is — its schema answers a bare
/// SOAP fault for `item:Flag` itself (measured 2026-11-24), while this tag answers lean and
/// follows what the web mail and IMAP's `\Flagged` set. The values are MAPI's numbering
/// ([MS-OXOFLAG], as Outlook's `OlFlagStatus`): 0 none, 1 a flag worked through, 2 flagged,
/// anything else no answer; the tag quoted as the server likes. A page that answers neither
/// door is silence, held (#1784).
#[test]
fn the_flag_status_tag_reads_the_flag_kerio_keeps() {
    for (tag, value, want) in [
        ("0x1090", "0", Some(FlagStatus::None)),
        ("0x1090", "1", Some(FlagStatus::Complete)),
        ("0x1090", "2", Some(FlagStatus::Flagged)),
        ("1090", "2", Some(FlagStatus::Flagged)),
        ("4240", "2", Some(FlagStatus::Flagged)),
        (" 4240 ", "2", Some(FlagStatus::Flagged)),
        ("0x1090", "3", None),
        ("0x1090", "twelve", None),
        ("1002", "2", None),
    ] {
        let xml = format!(
            "<t:Create><t:Message><t:ItemId Id=\"I1\" ChangeKey=\"C\"/><t:ExtendedProperty><t:ExtendedFieldURI PropertyTag=\"{tag}\" PropertyType=\"Integer\"/><t:Value>{value}</t:Value></t:ExtendedProperty></t:Message></t:Create>"
        );
        let got = page(&xml, 10).unwrap();
        let m = &got.messages[0].server;
        assert_eq!((m.flag, m.flag_said), (want.unwrap_or(FlagStatus::None), want.is_some()), "tag {tag}={value}");
    }
    // (The schema flag answers whenever its server has the flag family: the tag is a
    // fallback, never an override — what the node says wins, including saying no flag.)
    for (spelled, tag_value, want) in
        [("Complete", "2", FlagStatus::Complete), ("Flagged", "1", FlagStatus::Flagged), ("NotFlagged", "2", FlagStatus::None)]
    {
        let xml = format!(
            "<t:Create><t:Message><t:ItemId Id=\"I1\" ChangeKey=\"C\"/><t:Flag><t:FlagStatus>{spelled}</t:FlagStatus></t:Flag>\
             <t:ExtendedProperty><t:ExtendedFieldURI PropertyTag=\"0x1090\" PropertyType=\"Integer\"/><t:Value>{tag_value}</t:Value></t:ExtendedProperty></t:Message></t:Create>"
        );
        let got = page(&xml, 10).unwrap();
        assert_eq!(got.messages[0].server.flag, want, "the schema says {spelled} and the tag says {tag_value}");
    }
}

/// The answered and forwarded marks ride the last verb (`PR_LAST_VERB_EXECUTED`): 102
/// is a reply to the sender, 103 to all, 108 to the folder, 104 a forward ([MS-OXOMSG]'s
/// verbs); any other verb says neither mark, and a server that doesn't answer the tag
/// says nothing, which the merge holds rather than applies (#1784, the marks: measured
/// on Kerio 2026-11-24, a web-mail `\Answered` came back 102 and `\Forwarded` 104 —
/// the same door Outlook's replied arrow rides).
#[test]
fn the_last_verb_tag_brings_the_answered_and_forwarded_marks() {
    for (tag, value, answered, forwarded, said) in [
        ("0x1081", "102", true, false, true),
        ("1081", "102", true, false, true),
        ("4225", "104", false, true, true),
        ("0x1081", "103", true, false, true),
        ("0x1081", "108", true, false, true),
        ("0x1081", "105", false, false, true),
        ("0x1081", "twelve", false, false, false),
    ] {
        let xml = format!(
            "<t:Create><t:Message><t:ItemId Id=\"I1\" ChangeKey=\"C\"/><t:ExtendedProperty><t:ExtendedFieldURI PropertyTag=\"{tag}\" PropertyType=\"Integer\"/><t:Value>{value}</t:Value></t:ExtendedProperty></t:Message></t:Create>"
        );
        let got = page(&xml, 10).unwrap();
        let m = &got.messages[0].server;
        assert_eq!((m.answered, m.forwarded, m.marks_said), (answered, forwarded, said), "tag {tag}={value}");
    }
    // (No verb named at all: nothing said, so a mark held here for a send the server
    // never heard of survives the sync; keeping it is the merge's, not the reader's.)
    let got = page("<t:Create><t:Message><t:ItemId Id=\"I1\" ChangeKey=\"C\"/></t:Message></t:Create>", 10).unwrap();
    let m = &got.messages[0].server;
    assert!(!m.answered && !m.forwarded && !m.marks_said, "an answer that names no verb says nothing of the marks");
}

/// Every shape of the list asks over the `IdOnly` base with its properties named: the `Default`
/// base is what makes Kerio answer a listing with whole messages (measured 2026-11-24: 7 KB
/// against 287 MB over one mailbox), and a property named the wrong way is answered with a
/// refusal the asking never earned (`message:IsDraft` and the bare `0E67`, both measured the
/// same day). This test is the wire contract those measurements taught.
#[test]
fn every_shape_asks_the_light_base_with_its_properties_named() {
    for (shape, preview, recipients, flag) in [
        (Shape::Full, true, true, true),
        (Shape::Lean, false, true, false),
        (Shape::Bare, false, false, false),
        (Shape::Ids { flag: false }, false, false, false),
        (Shape::Ids { flag: true }, false, false, true),
    ] {
        for extended in [false, true] {
            let mut ask = SyncFolderItems::new("FOLDER", "", 200, shape);
            ask.extended = extended;
            let body = ask.element().write();
            assert!(body.contains("<t:BaseShape>IdOnly</t:BaseShape>"), "the {shape:?} asks the light base");
            assert!(!body.contains("Default"), "the {shape:?} never asks the base that weighs whole messages");
            assert!(
                body.contains("item:Subject")
                    && body.contains("message:IsRead")
                    && body.contains("item:IsDraft")
                    && body.contains("item:Categories")
                    && body.contains("item:HasAttachments"),
                "the {shape:?} names what a list shows: {body}"
            );
            assert!(!body.contains("message:IsDraft"), "…and names the draft state the way the schema has it");
            assert_eq!(body.contains("item:Preview"), preview, "the preview rides its shape alone");
            assert_eq!(body.contains("message:ToRecipients"), recipients, "the recipients ride their shapes");
            assert_eq!(body.contains("item:Flag"), flag, "the flag rides its shapes");
            assert!(body.contains("PropertyTag=\"0x0E67\"") == extended, "the MAPI tag rides the extended asking, spelled as the schema has it");
            assert!(
                body.contains("PropertyTag=\"0x1090\"") == extended,
                "and the MAPI flag tag rides with it: on Kerio it is the only flag there is"
            );
            assert!(body.contains("PropertyTag=\"0x1081\"") == extended, "and the verb tag with them: it carries the answered and forwarded marks");
            assert!(!body.contains("\"0E67\""), "and never the bare tag Kerio drops the connection for");
        }
    }
    let bare = SyncFolderItems::new("FOLDER", "", 200, Shape::Bare).element().write();
    let ids = SyncFolderItems::new("FOLDER", "", 200, Shape::Ids { flag: false }).element().write();
    assert_eq!(bare, ids, "`Bare` asks exactly the ids asking: the run's guard against a pointless fallback rests on these being the same bytes");
}
