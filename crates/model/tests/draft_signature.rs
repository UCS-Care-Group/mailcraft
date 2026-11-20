// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A draft remembers the HTML signature it was saved with: the signature as it was inserted, the
//! exact fragment last embedded in the draft and the Content-IDs its pictures were bound to. The
//! snapshot is local data, kept through a save and a reopen, and never authority on its own.

use mailcraft_model::draft_signature::{DraftSignature, MAX_CONTENT_ID, MAX_RENDERED};
use mailcraft_model::signature::{Details, MAX_HTML, MAX_TEXT};
use mailcraft_model::{Message, Signature};

/// The smallest PNG signature there is, and some bytes after it: enough for the model, which
/// reads only a picture's kind from its own first bytes, never its pixels.
fn png(extra: usize) -> Vec<u8> {
    let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
    bytes.resize(8 + extra, 7);
    bytes
}

/// A synthetic HTML signature with one picture, named by its `cid:` in the owner's HTML.
fn html_signature_with_logo() -> (Signature, String) {
    let mut sig = Signature::plain_text("Work", "Avery Quinn\nLumen Studio");
    let logo = sig.add_picture("logo.png", png(64), Some("Lumen logo"), Some(0)).unwrap();
    let html = format!(
        "<table><tr><td><img src=\"cid:sig-{logo}@signature.invalid\" alt=\"Lumen logo\"></td>\
         <td><b>Avery Quinn</b><br>Lumen Studio</td></tr></table>"
    );
    sig.set_html(Some(html.clone())).unwrap();
    (sig, html)
}

fn draft() -> Message {
    Message { id: 41, account: 1, folder: 2, subject: "Quarterly figures".into(), draft: true, ..Message::default() }
}

#[test]
fn a_saved_draft_keeps_its_signature_snapshot_through_a_reopen() {
    let (sig, _) = html_signature_with_logo();
    let mut file = serde_json::to_value(draft()).unwrap();
    let snapshot = serde_json::json!({
        "signature": serde_json::to_value(&sig).unwrap(),
        "rendered": "<div class=\"sig\"><img src=\"cid:part1.sig@draft.invalid\" alt=\"Lumen logo\"><b>Avery Quinn</b></div>",
        "contentIds": ["part1.sig@draft.invalid"],
    });
    file["draftSignature"] = snapshot.clone();

    let reopened: Message = serde_json::from_value(file).unwrap();
    let saved_again = serde_json::to_value(&reopened).unwrap();
    assert_eq!(saved_again.get("draftSignature"), Some(&snapshot), "the draft's signature snapshot must survive a save and a reopen");
}

/// A valid snapshot of an HTML signature with one picture, as a draft saves it.
fn snapshot_with_logo() -> DraftSignature {
    let (sig, _) = html_signature_with_logo();
    DraftSignature::new(sig, RENDERED.into(), vec![CID.into()]).expect("a valid one-picture snapshot")
}

const RENDERED: &str = "<div class=\"sig\"><img src=\"cid:part1.sig@draft.invalid\" alt=\"Lumen logo\"><b>Avery Quinn</b></div>";
const CID: &str = "part1.sig@draft.invalid";

/// An HTML signature without pictures.
fn html_signature(name: &str, html: &str) -> Signature {
    let mut sig = Signature::plain_text(name, "Avery Quinn");
    sig.set_html(Some(html.into())).unwrap();
    sig
}

/// An HTML signature holding two pictures.
fn html_signature_with_two_pictures() -> Signature {
    let mut sig = Signature::plain_text("Work", "Avery Quinn");
    sig.add_picture("logo.png", png(16), Some("Logo"), Some(0)).unwrap();
    sig.add_picture("badge.png", png(32), Some("Badge"), None).unwrap();
    sig.set_html(Some("<p><img src=\"cid:a\"><img src=\"cid:b\"></p>".into())).unwrap();
    assert_eq!(sig.pictures().len(), 2, "fixture: both pictures are kept");
    sig
}

#[test]
fn a_snapshot_without_pictures_round_trips_with_its_draft() {
    let sig = html_signature("Plain", "<p style=\"color:#123456\">Avery&nbsp;Quinn</p>");
    let snapshot = DraftSignature::new(sig.clone(), "<p>Avery Quinn</p>".into(), Vec::new()).expect("a valid zero-picture snapshot");
    assert_eq!(snapshot.signature(), &sig);
    assert_eq!(snapshot.rendered(), "<p>Avery Quinn</p>");
    assert!(snapshot.content_ids().is_empty());

    let back: DraftSignature = serde_json::from_value(serde_json::to_value(&snapshot).unwrap()).unwrap();
    assert_eq!(back, snapshot);

    let message = Message { draft_signature: Some(snapshot.clone()), ..draft() };
    let reopened: Message = serde_json::from_str(&serde_json::to_string(&message).unwrap()).unwrap();
    assert_eq!(reopened, message);
    assert_eq!(reopened.draft_signature, Some(snapshot));
}

#[test]
fn a_snapshot_with_a_png_keeps_its_raw_html_and_picture_bytes_exactly() {
    let (sig, html) = html_signature_with_logo();
    let snapshot = snapshot_with_logo();
    let message = Message { draft_signature: Some(snapshot.clone()), ..draft() };
    let reopened: Message = serde_json::from_slice(&serde_json::to_vec(&message).unwrap()).unwrap();
    let back = reopened.draft_signature.expect("the snapshot survives a save and a reopen");
    assert_eq!(back, snapshot);
    assert_eq!(back.signature().html(), Some(html.as_str()), "the owner's raw HTML, byte for byte");
    assert_eq!(back.rendered(), RENDERED);
    assert_eq!(back.content_ids(), [CID.to_string()].as_slice());
    let picture = back.signature().pictures().first().expect("its picture");
    assert_eq!(picture.data.as_slice(), png(64).as_slice(), "the picture's bytes, exactly");
    assert_eq!(back.signature().pictures(), sig.pictures());
}

#[test]
fn content_ids_must_match_the_pictures_one_for_one() {
    let (sig, _) = html_signature_with_logo();
    let with = |ids: &[&str]| DraftSignature::new(sig.clone(), RENDERED.into(), ids.iter().map(|s| s.to_string()).collect());
    assert!(with(&[CID]).is_some());
    assert_eq!(with(&[]), None, "a picture without its Content-ID");
    assert_eq!(with(&[CID, "part2.sig@draft.invalid"]), None, "a Content-ID without a picture");
    let plain = html_signature("Plain", "<p>Avery</p>");
    assert_eq!(DraftSignature::new(plain, "<p>Avery</p>".into(), vec![CID.into()]), None, "a Content-ID for a signature without pictures");
}

#[test]
fn duplicate_content_ids_are_refused() {
    let sig = html_signature_with_two_pictures();
    let snap = |a: &str, b: &str| DraftSignature::new(sig.clone(), "<p>sig</p>".into(), vec![a.into(), b.into()]);
    assert!(snap("one@draft.invalid", "two@draft.invalid").is_some());
    assert_eq!(snap("one@draft.invalid", "one@draft.invalid"), None, "two pictures under one Content-ID");
}

#[test]
fn malformed_content_ids_are_refused() {
    let (sig, _) = html_signature_with_logo();
    let with = |id: String| DraftSignature::new(sig.clone(), RENDERED.into(), vec![id]);
    let longest = "a".repeat(MAX_CONTENT_ID);
    assert!(with(longest.clone()).is_some(), "a Content-ID of exactly the most bytes");
    assert_eq!(with(format!("{longest}a")), None, "a Content-ID one byte over");
    for bad in
        ["", "<part1@draft.invalid>", "part1\"@draft.invalid", "part 1@draft.invalid", "part1@draft.invalid\r\n", "pärt1@draft.invalid", "\u{7f}"]
    {
        assert_eq!(with(bad.into()), None, "{bad:?} is not a Content-ID kept");
    }
}

#[test]
fn the_rendered_fragment_is_bounded_and_never_empty() {
    let sig = html_signature("Plain", "<p>Avery</p>");
    let with = |rendered: String| DraftSignature::new(sig.clone(), rendered, Vec::new());
    assert!(with("x".repeat(MAX_RENDERED)).is_some(), "exactly the most bytes");
    assert_eq!(with("x".repeat(MAX_RENDERED + 1)), None, "one byte over");
    assert_eq!(with(String::new()), None, "nothing rendered");
}

#[test]
fn only_a_clean_html_signature_within_its_caps_makes_a_snapshot() {
    let rich = Signature::plain_text("Work", "Avery Quinn");
    assert_eq!(DraftSignature::new(rich, "<p>Avery Quinn</p>".into(), Vec::new()), None, "not an HTML signature");
    for name in ["", " Work", "Work ", "Wo\u{7}rk"] {
        assert_eq!(DraftSignature::new(html_signature(name, "<p>x</p>"), "<p>x</p>".into(), Vec::new()), None, "name {name:?}");
    }
    let mut detailed = html_signature("Work", "<p>x</p>");
    detailed.details = Some(Details { name: " Avery ".into(), ..Details::default() });
    assert_eq!(DraftSignature::new(detailed.clone(), "<p>x</p>".into(), Vec::new()), None, "details not as cleaned");
    detailed.details = Some(Details { name: "Avery".into(), ..Details::default() });
    assert!(DraftSignature::new(detailed, "<p>x</p>".into(), Vec::new()).is_some(), "clean details");

    let longest = html_signature("Work", &"x".repeat(MAX_HTML));
    assert!(DraftSignature::new(longest, "<p>x</p>".into(), Vec::new()).is_some(), "HTML of exactly the most bytes");

    let full = {
        let mut sig = Signature::plain_text("Work", &"y".repeat(MAX_TEXT));
        sig.set_html(Some("<p>y</p>".into())).unwrap();
        sig
    };
    assert_eq!(full.body().len(), MAX_TEXT, "fixture: text of exactly the most characters");
    assert!(DraftSignature::new(full, "<p>y</p>".into(), Vec::new()).is_some());

    // A signature's own reader takes text of any length: a snapshot doesn't.
    let over: Signature =
        serde_json::from_value(serde_json::json!({ "name": "Work", "text": "y".repeat(MAX_TEXT + 1), "html": "<p>y</p>" })).unwrap();
    assert!(over.body().len() > MAX_TEXT, "fixture: text over the cap");
    assert_eq!(DraftSignature::new(over, "<p>y</p>".into(), Vec::new()), None);
}

/// A valid snapshot as JSON, to be spoiled.
fn snapshot_json() -> serde_json::Value {
    serde_json::to_value(snapshot_with_logo()).unwrap()
}

#[test]
fn a_snapshot_whose_signature_would_lose_or_change_a_picture_is_not_read() {
    assert!(serde_json::from_value::<DraftSignature>(snapshot_json()).is_ok(), "fixture: the unspoiled snapshot reads");

    let mut twice = snapshot_json();
    let first = twice["signature"]["pictures"][0].clone();
    twice["signature"]["pictures"].as_array_mut().unwrap().push(first);
    let mut not_a_picture = snapshot_json();
    not_a_picture["signature"]["pictures"][0]["data"] = "bm90IGEgcGljdHVyZQ==".into();
    let mut unused = snapshot_json();
    let mut extra = unused["signature"]["pictures"][0].clone();
    extra["id"] = 9.into();
    unused["signature"]["pictures"].as_array_mut().unwrap().push(extra);
    unused["contentIds"].as_array_mut().unwrap().push("part9.sig@draft.invalid".into());
    let mut wrong_kind = snapshot_json();
    wrong_kind["signature"]["pictures"][0]["mime"] = "image/gif".into();
    let mut html_over = snapshot_json();
    html_over["signature"]["html"] = "x".repeat(MAX_HTML + 1).into();

    for (what, spoiled) in [
        ("a picture twice", twice),
        ("bytes that aren't a picture", not_a_picture),
        ("a picture the text doesn't hold", unused),
        ("a kind its bytes don't say", wrong_kind),
        ("HTML over the cap", html_over),
    ] {
        // The signature's own reader still makes something of it...
        assert!(serde_json::from_value::<Signature>(spoiled["signature"].clone()).is_ok(), "fixture: {what}");
        // ...but not a snapshot.
        assert!(serde_json::from_value::<DraftSignature>(spoiled.clone()).is_err(), "{what}");
    }
}

#[test]
fn a_draft_from_before_has_no_snapshot() {
    let file = serde_json::to_value(draft()).unwrap();
    assert!(file.get("draftSignature").is_none(), "nothing is written for a draft without one");
    let reopened: Message = serde_json::from_value(file).unwrap();
    assert_eq!(reopened.draft_signature, None);
    assert_eq!(reopened, draft());
}

#[test]
fn a_spoiled_snapshot_is_dropped_and_the_draft_still_opens() {
    let mut duplicate_ids = snapshot_json();
    duplicate_ids["signature"] = serde_json::to_value(html_signature_with_two_pictures()).unwrap();
    duplicate_ids["contentIds"] = serde_json::json!(["one@draft.invalid", "one@draft.invalid"]);
    let mut empty_rendered = snapshot_json();
    empty_rendered["rendered"] = "".into();
    let spoiled =
        [serde_json::json!("nonsense"), serde_json::json!(null), serde_json::json!({ "rendered": RENDERED }), duplicate_ids, empty_rendered];
    for value in spoiled {
        let mut file = serde_json::to_value(draft()).unwrap();
        file["draftSignature"] = value.clone();
        let reopened: Message = serde_json::from_value(file).unwrap_or_else(|e| panic!("the draft opens despite {value}: {e}"));
        assert_eq!(reopened, draft(), "{value}");
    }
}

#[test]
fn debug_tells_sizes_never_the_signature() {
    let marker = "Zq9SyntheticMarker";
    let mut sig = Signature::plain_text(format!("Work {marker}"), &format!("Avery {marker}"));
    sig.add_picture("logo.png", png(8), Some(marker), Some(0)).unwrap();
    sig.set_html(Some(format!("<p title=\"{marker}\">Avery</p>"))).unwrap();
    let snapshot = DraftSignature::new(sig, format!("<p class=\"{marker}\">Avery</p>"), vec![format!("{marker}@draft.invalid")]).unwrap();
    let shown = format!("{snapshot:?}");
    assert!(!shown.contains(marker), "{shown}");
    assert!(!shown.contains("<p"), "{shown}");
    let message = format!("{:?}", Message { draft_signature: Some(snapshot), ..draft() });
    assert!(!message.contains(marker));
}
