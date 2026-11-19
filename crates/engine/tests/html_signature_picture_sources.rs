// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An HTML signature names its pictures as `signature.import` lets it: by a web address, which
//! stays as it is, or by the `cid:` address of a picture the signature owns. What is kept in the
//! mailbox is judged the same way, so that what the owner keeps is what can be sent:
//!
//! * a file's name or path (`other.png`, `./images/other.png`), a `file:` address, another scheme
//!   or a `//host` address names a file that is not beside the signature, and a `cid:` the
//!   signature does not own names a picture it doesn't have: `signature.save` of the open
//!   signature refuses either, whether or not the signature owns pictures, and so does
//!   `signature.html` written straight to a signature that is not open;
//! * the open editor takes such HTML as it takes HTML with an email-compatibility error (the
//!   window sends the HTML source through `signature.html` as it is typed), except a `cid:` of an
//!   imported signature's that it does not own, which it refuses as before;
//! * a refusal leaves the mailbox and the open editor exactly as they were;
//! * an `https:` address and the signature's own picture are kept and saved as before, and a
//!   `data:` picture stays the email-compatibility checker's to refuse at the save (OL-IMG-004);
//! * an address is read as the importer reads it ([`picture_source`]): a web address with white
//!   space around it or its scheme in capitals is still a web address, `CID:` names the
//!   signature's own picture as `cid:` does, and a local address hidden behind character
//!   references is still a local one;
//! * HTML whose pictures can't be listed (a `<base>` element, more than 256 pictures) is refused
//!   saying which, so that the owner knows what to change.
//!
//! Each loop records what every address did before it judges, so that a failure names them all.
//! The people, hosts and picture are invented; nothing here reads a file, fetches or sends.

use mailcraft_engine::cmd::base64_encode;
use mailcraft_engine::mime::signature_import::{PictureSource, picture_source};
use mailcraft_engine::{CmdError, Session, sample};
use mailcraft_html::compat::{self, Rule, Severity};
use mailcraft_time::{Date, DateTime};
use serde_json::{Value, json};

/// A one-pixel PNG, whole and valid: the fixture the other signature import tests use.
const ONE_PIXEL_PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,
    0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0a, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

/// The owner's file: its words and one local picture, sized by its attributes (nothing to fix).
const IMPORTED: &str = r#"<p>Ana Pereira - Lisbon Bindery</p><p><img src="bindery-mark.png" alt="Bindery mark" width="120" height="80"></p>"#;
const NAME: &str = "Bindery";
/// The words the signature keeps.
const WORDS: &str = "Ana Pereira - Lisbon Bindery";

/// The refusal of a picture that is neither on the web nor one of the signature's own, and what
/// to do about it.
const LOCAL_PICTURE: &str = "this HTML names a picture that is neither on the web nor one of this signature's own: give it a web address (https://…), or name the picture's file relative to the HTML file and bring both in with Import HTML…";
/// The refusal of HTML naming a picture by a `cid:` address the signature doesn't own.
const FOREIGN_PICTURE: &str = "this HTML names a picture this signature doesn't have";
/// The compat gate's refusal, the same for any count of errors.
const ERRORS: &str = "this signature has email compatibility errors to fix before it can be saved";

/// Picture addresses that name a file the signature doesn't carry, or some other scheme.
const LOCAL_SOURCES: [&str; 9] = [
    "other.png",
    "./images/other.png",
    "../other.png",
    "/home/ana/badge.png",
    "file:///home/ana/badge.png",
    "FILE:///C:/Users/ana/badge.png",
    "ftp://pictures.example/badge.png",
    "//pictures.example/badge.png",
    "javascript:alert(1)",
];
/// `cid:` addresses no signature here owns: someone else's, and an internal one not given out.
const FOREIGN_CIDS: [&str; 2] = ["cid:badge@pictures.example", "cid:sig-99@signature.invalid"];

fn session() -> Session {
    let now = DateTime::new(Date::new(2026, 11, 17).unwrap(), 10, 0, 0).unwrap().to_timestamp(0);
    Session::at(sample::mailbox(now, 0), now, 0)
}

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

/// What a command did, in words a test compares: its refusal's English text, or "accepted"; and
/// whether the mailbox and the open editor are exactly as they were before it.
fn judged(s: &mut Session, id: &str, params: Value) -> (String, bool) {
    let (mailbox, editor) = (s.mailbox.clone(), s.signature_edit.clone());
    let said = match s.run(id, &params) {
        Ok(_) => "accepted".to_string(),
        Err(e @ (CmdError::Unknown(_) | CmdError::Params(_) | CmdError::Disabled(_))) => format!("refused for another reason: {e}"),
        Err(e) => e.to_string(),
    };
    (said, s.mailbox == mailbox && s.signature_edit == editor)
}

/// What every address in `sources` is to come to: refused with `said`, nothing changed.
fn all_refused(sources: &[&'static str], said: &str) -> Vec<(&'static str, (String, bool))> {
    sources.iter().map(|src| (*src, (said.to_string(), true))).collect()
}

/// `html` with one more picture, at `src`, sized by its attributes (nothing for the checker).
fn with_picture(html: &str, src: &str) -> String {
    format!(r#"{html}<p><img src="{src}" alt="Badge" width="40" height="40"></p>"#)
}

/// A session with [`IMPORTED`] imported (its picture bound) and opened in the editor; the stored
/// HTML (with the picture's internal address) and that address.
fn imported_and_open() -> (Session, String, String) {
    let mut s = session();
    run(
        &mut s,
        "signature.import",
        json!({"name": NAME, "html": IMPORTED, "assets": [{"name": "bindery-mark.png", "data": base64_encode(ONE_PIXEL_PNG)}]}),
    );
    let sig = s.mailbox.signature(NAME).expect("the imported signature");
    assert_eq!(sig.pictures().len(), 1, "fixture guard: one picture bound");
    let html = sig.html().expect("fixture guard: an HTML signature").to_string();
    let cid = mailcraft_engine::mime::rich::internal_cid(&sig.pictures()[0]);
    assert!(html.contains(&format!("cid:{cid}")), "fixture guard: the HTML names the picture by its internal address: {html}");
    assert!(compat::check(&html).expect("checked").iter().all(|f| f.severity != Severity::Error), "fixture guard: nothing to fix: {html}");
    run(&mut s, "signature.open", json!({"name": NAME}));
    (s, html, cid)
}

/// The open signature made `html` in the editor (taken, and a change to save), then saved: what
/// the save did.
fn edited_then_saved(s: &mut Session, html: &str) -> (String, bool) {
    run(s, "signature.html", json!({"html": html}));
    let e = s.signature_edit.as_ref().expect("the signature is open");
    assert_eq!(e.signature.html(), Some(html), "the editor takes it");
    assert!(e.dirty, "as a change to save");
    judged(s, "signature.save", json!({}))
}

#[test]
fn an_open_imported_signature_refuses_a_picture_neither_on_the_web_nor_its_own() {
    let (mut s, html, cid) = imported_and_open();
    let saved = s.mailbox.signature(NAME).cloned().expect("imported");
    let got: Vec<_> = LOCAL_SOURCES.iter().map(|src| (*src, edited_then_saved(&mut s, &with_picture(&html, src)))).collect();
    assert_eq!(got, all_refused(&LOCAL_SOURCES, LOCAL_PICTURE), "each save is refused, saying why, and changes nothing");
    assert_eq!(s.mailbox.signature(NAME), Some(&saved), "the signature is as it was imported");
    // A `cid:` it doesn't own is refused as soon as the editor is given it, as before.
    let got: Vec<_> = FOREIGN_CIDS.iter().map(|src| (*src, judged(&mut s, "signature.html", json!({"html": with_picture(&html, src)})))).collect();
    assert_eq!(got, all_refused(&FOREIGN_CIDS, FOREIGN_PICTURE), "each edit is refused, saying why, and changes nothing");
    // Back to the signature's own HTML, the save keeps it with its own picture bound.
    run(&mut s, "signature.html", json!({"html": html}));
    run(&mut s, "signature.save", json!({}));
    let sig = s.mailbox.signature(NAME).expect("saved");
    assert_eq!(sig.html(), Some(html.as_str()), "the HTML as it was");
    assert_eq!(sig.pictures().len(), 1, "its picture kept");
    assert_eq!(sig.pictures()[0].data.as_slice(), ONE_PIXEL_PNG, "with its bytes");
    assert_eq!(mailcraft_engine::mime::rich::internal_cid(&sig.pictures()[0]), cid, "under its address");
}

#[test]
fn an_open_html_signature_without_pictures_refuses_them_too() {
    let mut s = session();
    run(&mut s, "signature.set", json!({"name": "Card", "text": "Ana Pereira\nLisbon Bindery"}));
    run(&mut s, "signature.open", json!({"name": "Card"}));
    run(&mut s, "signature.html", json!({"html": "<p>Ana Pereira</p>"}));
    run(&mut s, "signature.save", json!({}));
    let saved = s.mailbox.signature("Card").cloned().expect("saved");
    assert!(saved.pictures().is_empty() && saved.html().is_some(), "fixture guard: an HTML signature that owns no picture");
    let got: Vec<_> =
        LOCAL_SOURCES.iter().chain(&FOREIGN_CIDS).map(|src| (*src, edited_then_saved(&mut s, &with_picture("<p>Ana Pereira</p>", src)))).collect();
    let want = [all_refused(&LOCAL_SOURCES, LOCAL_PICTURE), all_refused(&FOREIGN_CIDS, FOREIGN_PICTURE)].concat();
    assert_eq!(got, want, "each save is refused, saying why, and changes nothing");
    assert_eq!(s.mailbox.signature("Card"), Some(&saved), "the signature as it was saved");
    run(&mut s, "signature.close", json!({"discard": true}));

    // A rich signature made HTML in the editor is judged the same way.
    run(&mut s, "signature.set", json!({"name": "Rich", "text": "Ana Pereira"}));
    run(&mut s, "signature.open", json!({"name": "Rich"}));
    let got = edited_then_saved(&mut s, &with_picture("<p>Ana Pereira</p>", "other.png"));
    assert_eq!(got, (LOCAL_PICTURE.to_string(), true), "a rich signature made HTML");
    assert_eq!(s.mailbox.signature("Rich").and_then(|x| x.html()), None, "the rich one still rich in the mailbox");
}

#[test]
fn html_written_straight_to_a_signature_that_is_not_open_is_refused_the_same_way() {
    let mut s = session();
    run(&mut s, "signature.set", json!({"name": "Card", "text": "Ana Pereira\nLisbon Bindery"}));
    run(&mut s, "signature.set", json!({"name": "Other", "text": "Ana Pereira"}));
    run(&mut s, "signature.open", json!({"name": "Other"}));
    run(&mut s, "compose.type", json!({"text": "Unsaved words for the other signature"}));
    assert!(s.signature_edit.as_ref().is_some_and(|e| e.dirty), "fixture guard: the open editor has unsaved words");
    let straight = |s: &mut Session, src: &'static str| {
        (src, judged(s, "signature.html", json!({"name": "Card", "html": with_picture("<p>Ana Pereira</p>", src)})))
    };
    let got: Vec<_> = LOCAL_SOURCES.iter().chain(&FOREIGN_CIDS).map(|src| straight(&mut s, src)).collect();
    let want = [all_refused(&LOCAL_SOURCES, LOCAL_PICTURE), all_refused(&FOREIGN_CIDS, FOREIGN_PICTURE)].concat();
    assert_eq!(got, want, "each write is refused, saying why, and changes nothing");
    assert_eq!(s.mailbox.signature("Card").and_then(|x| x.html()), None, "the signature is still the rich one");
}

#[test]
fn a_web_picture_and_the_signatures_own_picture_are_kept() {
    let web = "https://pictures.example/badge.png";
    // The open imported signature: its own picture, with a web one beside it.
    let (mut s, html, cid) = imported_and_open();
    let edited = with_picture(&html.replace(WORDS, "Ana Pereira - Lisbon Bindery, Porto"), web);
    run(&mut s, "signature.html", json!({"html": edited}));
    run(&mut s, "signature.save", json!({}));
    let sig = s.mailbox.signature(NAME).expect("saved");
    assert_eq!(sig.html(), Some(edited.as_str()), "the HTML as edited");
    assert_eq!(sig.pictures().len(), 1, "its own picture kept");
    assert_eq!(mailcraft_engine::mime::rich::internal_cid(&sig.pictures()[0]), cid, "under its address");
    assert_eq!(sig.body().pictures(), vec![sig.pictures()[0].id], "standing in its text");

    // An open signature without pictures, and one written straight to the mailbox.
    let mut s = session();
    run(&mut s, "signature.set", json!({"name": "Card", "text": "Ana Pereira"}));
    run(&mut s, "signature.set", json!({"name": "Straight", "text": "Ana Pereira"}));
    run(&mut s, "signature.open", json!({"name": "Card"}));
    let page = with_picture("<p>Ana Pereira</p>", web);
    run(&mut s, "signature.html", json!({"html": page}));
    run(&mut s, "signature.save", json!({}));
    assert_eq!(s.mailbox.signature("Card").and_then(|x| x.html()), Some(page.as_str()), "kept from the editor");
    run(&mut s, "signature.html", json!({"name": "Straight", "html": page}));
    assert_eq!(s.mailbox.signature("Straight").and_then(|x| x.html()), Some(page.as_str()), "kept straight in the mailbox");
}

#[test]
fn a_data_picture_stays_the_checkers_to_refuse() {
    let data = format!("data:image/png;base64,{}", base64_encode(ONE_PIXEL_PNG));
    let page = with_picture("<p>Ana Pereira</p>", &data);
    let found = compat::check(&page).expect("checked");
    let errors: Vec<_> = found.iter().filter(|f| f.severity == Severity::Error).collect();
    assert_eq!(errors.len(), 1, "fixture guard: one error: {found:?}");
    assert_eq!(errors[0].rule, Rule::DataUriImage, "fixture guard: OL-IMG-004");

    // Open: the editor takes it, so that the checker's fix can be applied there; the save waits.
    let mut s = session();
    run(&mut s, "signature.set", json!({"name": "Card", "text": "Ana Pereira"}));
    run(&mut s, "signature.open", json!({"name": "Card"}));
    assert_eq!(edited_then_saved(&mut s, &page), (ERRORS.to_string(), true), "the save is refused by the checker");

    // Straight to the mailbox: the checker's refusal.
    run(&mut s, "signature.close", json!({"discard": true}));
    assert_eq!(judged(&mut s, "signature.html", json!({"name": "Card", "html": page})), (ERRORS.to_string(), true), "refused by the checker");
}

/// Web addresses spelled otherwise than plainly: white space around them, the scheme in capitals.
const WEB_SPELLINGS: [&str; 3] =
    ["  https://pictures.example/badge.png  ", "\n\thttps://pictures.example/badge.png\n", "HTTPS://PICTURES.EXAMPLE/badge.png"];
/// A local address written with character references, which the HTML reads as `file:` and `./`.
const ENCODED_LOCAL: [&str; 2] = ["&#x66;ile:///home/ana/badge.png", "&#46;&#47;images/other.png"];

#[test]
fn addresses_are_read_as_the_importer_reads_them() {
    // The importer's own reading, which every path below is to agree with.
    for src in WEB_SPELLINGS {
        assert_eq!(picture_source(src), PictureSource::Web, "{src:?} is a web address to the importer");
    }
    assert_eq!(picture_source("CID:sig-1@signature.invalid"), PictureSource::Cid("sig-1@signature.invalid"), "CID: in capitals");
    assert_eq!(picture_source("file:///home/ana/badge.png"), PictureSource::Local, "what the encoded address reads as");

    // The open imported signature: a web picture spelled otherwise is kept, and so is its own
    // picture named with `CID:` in capitals; a local address behind character references is
    // refused and changes nothing.
    let (mut s, html, cid) = imported_and_open();
    let mut got: Vec<(String, (String, bool))> = Vec::new();
    for src in WEB_SPELLINGS {
        got.push((src.to_string(), edited_then_saved(&mut s, &with_picture(&html, src))));
    }
    let own_in_capitals = html.replace(&format!("cid:{cid}"), &format!("CID:{cid}"));
    assert_ne!(own_in_capitals, html, "fixture guard: the HTML names its picture by `cid:`");
    got.push(("CID:".to_string(), edited_then_saved(&mut s, &own_in_capitals)));
    let kept = s.mailbox.signature(NAME).cloned().expect("imported");
    for src in ENCODED_LOCAL {
        got.push((src.to_string(), edited_then_saved(&mut s, &with_picture(&html, src))));
    }
    let accepted = ("accepted".to_string(), false);
    let refused = (LOCAL_PICTURE.to_string(), true);
    let want: Vec<(String, (String, bool))> = WEB_SPELLINGS
        .iter()
        .map(|src| (src.to_string(), accepted.clone()))
        .chain([("CID:".to_string(), accepted.clone())])
        .chain(ENCODED_LOCAL.iter().map(|src| (src.to_string(), refused.clone())))
        .collect();
    assert_eq!(got, want, "the open imported signature");
    let sig = s.mailbox.signature(NAME).expect("saved");
    assert_eq!(sig, &kept, "the refusals left the last save as it was");
    assert_eq!(sig.html(), Some(own_in_capitals.as_str()), "its own picture named with CID: is kept as written");
    assert_eq!(sig.pictures().len(), 1, "with its picture");
    assert_eq!(sig.body().pictures(), vec![sig.pictures()[0].id], "standing in its text");

    // An open signature without pictures, and HTML written straight to one that is not open.
    let mut s = session();
    run(&mut s, "signature.set", json!({"name": "Card", "text": "Ana Pereira"}));
    run(&mut s, "signature.set", json!({"name": "Straight", "text": "Ana Pereira"}));
    run(&mut s, "signature.open", json!({"name": "Card"}));
    let mut got: Vec<(String, String, (String, bool))> = Vec::new();
    for src in WEB_SPELLINGS.iter().chain(&ENCODED_LOCAL) {
        let page = with_picture("<p>Ana Pereira</p>", src);
        got.push(("open".to_string(), src.to_string(), edited_then_saved(&mut s, &page)));
        got.push(("straight".to_string(), src.to_string(), judged(&mut s, "signature.html", json!({"name": "Straight", "html": page}))));
    }
    let want: Vec<(String, String, (String, bool))> = WEB_SPELLINGS
        .iter()
        .map(|src| (*src, &accepted))
        .chain(ENCODED_LOCAL.iter().map(|src| (*src, &refused)))
        .flat_map(|(src, said)| [("open".to_string(), src.to_string(), said.clone()), ("straight".to_string(), src.to_string(), said.clone())])
        .collect();
    assert_eq!(got, want, "an open signature without pictures, and one written straight to the mailbox");
}

/// The refusals of HTML whose pictures can't be listed, saying why.
const BASE_ELEMENT: &str = "this signature's HTML could not be read for its pictures: remove its <base> element";
const TOO_MANY: &str = "this signature's HTML could not be read for its pictures: it may name at most 256 pictures";

#[test]
fn html_whose_pictures_cant_be_listed_is_refused_saying_why() {
    let base = r#"<base href="https://pictures.example/"><p>Ana Pereira</p><p><img src="https://pictures.example/badge.png" alt="Badge" width="40" height="40"></p>"#;
    let many: String = std::iter::once("<p>Ana Pereira</p>".to_string())
        .chain((0..257).map(|i| format!(r#"<img src="https://pictures.example/{i}.png" alt="" width="16" height="16">"#)))
        .collect();
    for page in [base, many.as_str()] {
        assert!(
            compat::check(page).expect("checked").iter().all(|f| f.severity != Severity::Error),
            "fixture guard: nothing for the checker to refuse, so that the pictures are what is judged"
        );
    }
    let mut s = session();
    run(&mut s, "signature.set", json!({"name": "Card", "text": "Ana Pereira"}));
    run(&mut s, "signature.set", json!({"name": "Straight", "text": "Ana Pereira"}));
    run(&mut s, "signature.open", json!({"name": "Card"}));
    let got = [
        edited_then_saved(&mut s, base),
        judged(&mut s, "signature.html", json!({"name": "Straight", "html": base})),
        edited_then_saved(&mut s, &many),
        judged(&mut s, "signature.html", json!({"name": "Straight", "html": many})),
    ];
    let want = [(BASE_ELEMENT.to_string(), true), (BASE_ELEMENT.to_string(), true), (TOO_MANY.to_string(), true), (TOO_MANY.to_string(), true)];
    assert_eq!(got, want, "each is refused, saying what to change, and changes nothing");
}
