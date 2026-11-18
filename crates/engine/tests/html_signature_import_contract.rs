// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! `signature.import`'s door (#R4.5): the fields it takes and no others, the types they have, the
//! caps a picture file is refused by before anything of it is decoded, and a refusal that leaves
//! the mailbox, the signature open in the editor and the account's defaults exactly as they were.
//!
//! The document and its pictures are synthetic: the picture is the header-valid quota fixture,
//! filled out to the size it is tested at, and says nothing about pixels. Nothing here reads a
//! file, opens a dialog or sends anything; the end-to-end import-to-draft binding is the separate
//! tracer (`html_signature_import.rs`), which this file does not weaken or replace.

use mailcraft_engine::cmd::base64_encode;
use mailcraft_engine::{CmdError, Session, sample};
use mailcraft_mime_model::rich::internal_cid;
use mailcraft_mime_model::signature_import::MAX_ASSET_NAME;
use mailcraft_model::signature::{Layout, MAX_HTML, MAX_PICTURE_BYTES, MAX_PICTURES, MAX_PICTURES_BYTES};
use mailcraft_time::{Date, DateTime};
use serde_json::{Value, json};

/// A one-pixel PNG, whole and valid: a picture by its own first bytes, the same fixture the MIME
/// integration tests use.
const ONE_PIXEL_PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,
    0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0a, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

/// The picture's file name beside the owner's document, and the one its `<img>` names twice.
const PICTURE_FILE: &str = "studio-mark.png";

/// Most base64 characters one picture's `data` may hold once its white space is gone, as the door
/// works it out from the model's own per-picture cap (`ceil(bytes / 3) * 4`).
const MAX_ENCODED: usize = MAX_PICTURE_BYTES.saturating_add(2) / 3 * 4;

fn session() -> (Session, u64) {
    let now = DateTime::new(Date::new(2026, 10, 14).unwrap(), 16, 0, 0).unwrap().to_timestamp(0);
    let s = Session::at(sample::mailbox(now, 0), now, 0);
    let account = s.mailbox.default_account().expect("the sample mailbox has an account").id;
    (s, account)
}

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

/// What was refused, and never an `Ok`.
fn refused(result: Result<Value, CmdError>, label: &str) -> CmdError {
    match result {
        Ok(v) => panic!("{label} was not refused: {v}"),
        Err(e) => e,
    }
}

/// A refusal for the parameters themselves: what a caller can see and correct.
fn is_bad_params(e: &CmdError) -> bool {
    matches!(e, CmdError::Params(_))
}

/// `bytes` as base64 the way a mail writer breaks it: a CRLF every 76 characters.
fn mime_base64(bytes: &[u8]) -> String {
    let flat = base64_encode(bytes);
    let mut out = String::with_capacity(flat.len().saturating_add(flat.len() / 76 * 2).saturating_add(2));
    for (at, c) in flat.chars().enumerate() {
        if at > 0 && at % 76 == 0 {
            out.push_str("\r\n");
        }
        out.push(c);
    }
    out
}

/// One supplied picture file: its name as the document writes it, and its bytes.
fn asset(name: &str, bytes: &[u8]) -> Value {
    json!({"name": name, "data": mime_base64(bytes)})
}

/// A document that stands one picture in each of the files named, in a table of its own.
fn document(files: &[&str]) -> String {
    let mut html = String::from("<table width=\"520\" cellpadding=\"0\" cellspacing=\"0\" border=\"0\" style=\"width:520px\"><tr><td>");
    for file in files {
        html.push_str(&format!("<div><img src=\"{file}\" alt=\"A mark\" width=\"120\" height=\"80\" style=\"border:0\"></div>"));
    }
    html.push_str("</td></tr></table>");
    html
}

/// A picture the size a test needs: the header-valid quota fixture's own first bytes, filled to
/// `len`. It is no claim about pixels, only about the bytes a cap counts.
fn picture_of(len: usize) -> Vec<u8> {
    let mut bytes = ONE_PIXEL_PNG.to_vec();
    bytes.resize(len, 0);
    bytes
}

/// The owner's own document: two tables, one inside the other, their look in `style` attributes,
/// and the picture named by its file name in two places.
fn own_html() -> &'static str {
    r#"<table width="520" cellpadding="0" cellspacing="0" border="0" style="width:520px;background-color:#fdf6e3;padding:12px;font-family:Georgia,serif"><tr>
  <td style="padding-right:16px"><img src="studio-mark.png" alt="Northwind mark" width="120" height="80" style="border:0"></td>
  <td style="font-family:Georgia,serif"><table cellpadding="0" cellspacing="0" border="0" style="background-color:#ffffff;padding:8px"><tr>
    <td><div>Priya Raman - Northwind Atelier</div><div>Studio Two, Harbour Lane</div></td>
  </tr></table></td>
</tr></table>
<div><img src="studio-mark.png" alt="Northwind mark" width="120" height="80" style="border:0"></div>"#
}

#[test]
fn an_imported_signature_is_stored_whole_and_leaves_a_dirty_editor_as_it_was() {
    let (mut s, account) = session();
    // Existing commands only: two signatures, the account's default, one open in the editor with
    // unsaved words the import must not touch even though it succeeds.
    run(&mut s, "signature.set", json!({"name": "Standard", "text": "Priya Raman\nNorthwind Atelier"}));
    run(&mut s, "signature.set", json!({"name": "Studio", "text": "Priya Raman\nStudio Two, Harbour Lane"}));
    run(&mut s, "signature.defaults", json!({"account": account, "new": "Standard"}));
    run(&mut s, "signature.open", json!({"name": "Studio"}));
    run(&mut s, "compose.type", json!({"text": "Unsaved words for the studio signature"}));
    let editor_before = s.signature_edit.clone().expect("a signature is open");
    assert!(editor_before.dirty, "the fixture's editor has unsaved words");
    let default_before = s.mailbox.account(account).expect("the account").signature.clone();
    let count_before = s.mailbox.signatures.len();

    let answer =
        run(&mut s, "signature.import", json!({"name": " Northwind Studio ", "html": own_html(), "assets": [asset(PICTURE_FILE, ONE_PIXEL_PNG)]}));

    // Its clean name and nothing else.
    assert_eq!(answer, json!({"name": "Northwind Studio"}), "the import answers the signature's clean name and nothing else");
    // The editing commands' state is the owner's: not opened, not replaced, not made clean.
    assert_eq!(s.signature_edit.as_ref(), Some(&editor_before), "the open signature, its unsaved words and its selection are as they were");
    assert!(s.compose.is_none(), "the import opened nothing to write");
    assert_eq!(s.mailbox.account(account).expect("the account").signature, default_before, "the account's default is untouched");
    assert_eq!(s.mailbox.signatures.len(), count_before + 1, "one signature was added");

    // What the mailbox kept: the owner's document byte for byte but for the picture's address,
    // and the picture once, with its own name and bytes, wherever the document names it.
    let stored = s.mailbox.signature("Northwind Studio").expect("the import put the signature in the mailbox");
    assert_eq!(stored.layout, Layout::Inline, "an imported document lays its pictures out in its own tables");
    let pictures = stored.pictures();
    assert_eq!(pictures.len(), 1, "one picture for the two places the document names it: {pictures:?}");
    let picture = pictures.first().expect("the picture just counted");
    assert_eq!(picture.name, PICTURE_FILE, "the picture is named by its file's own name");
    assert_eq!(picture.data.as_slice(), ONE_PIXEL_PNG, "the picture's bytes are kept as they are");
    let expected = own_html().replace(&format!("src=\"{PICTURE_FILE}\""), &format!("src=\"cid:{}\"", internal_cid(picture)));
    assert_eq!(stored.html(), Some(expected.as_str()), "the document is kept as written, only the picture's address changed");
    let stand_in = stored.body().text();
    assert!(stand_in.contains("Priya Raman - Northwind Atelier"), "the text beside the HTML keeps the owner's words: {stand_in:?}");
}

#[test]
fn an_import_of_a_name_the_mailbox_keeps_is_refused_whatever_its_case() {
    let (mut s, _account) = session();
    run(&mut s, "signature.set", json!({"name": "Studio", "text": "Priya Raman"}));
    let before = s.mailbox.clone();

    let said = refused(
        s.run("signature.import", &json!({"name": " sTuDiO ", "html": document(&[PICTURE_FILE]), "assets": [asset(PICTURE_FILE, ONE_PIXEL_PNG)]})),
        "an import of a name the mailbox keeps",
    );

    assert!(!matches!(said, CmdError::Unknown(_)), "the command is there, and refused for its own reason: {said}");
    assert!(said.to_string().contains("already a signature named"), "the refusal says the name is taken: {said}");
    assert!(s.mailbox == before, "the refused import left the mailbox as it was");
    assert_eq!(s.mailbox.signature("Studio").map(|sig| sig.html()), Some(None), "the signature of that name was not replaced by an HTML one");
}

#[test]
fn an_import_of_the_name_open_in_the_editor_is_refused_even_with_unsaved_words() {
    let (mut s, _account) = session();
    run(&mut s, "signature.set", json!({"name": "Studio", "text": "Priya Raman"}));
    run(&mut s, "signature.open", json!({"name": "Studio"}));
    run(&mut s, "compose.type", json!({"text": "Unsaved words"}));
    let editor_before = s.signature_edit.clone().expect("a signature is open");
    let before = s.mailbox.clone();

    let said = refused(
        s.run("signature.import", &json!({"name": "STUDIO", "html": document(&[PICTURE_FILE]), "assets": [asset(PICTURE_FILE, ONE_PIXEL_PNG)]})),
        "an import of the open signature's name",
    );

    assert!(said.to_string().contains("already a signature named"), "the refusal names the name that is taken: {said}");
    assert!(s.mailbox == before, "the refused import left the mailbox as it was");
    assert_eq!(s.signature_edit.as_ref(), Some(&editor_before), "the open signature and its unsaved words are as they were");
    assert!(editor_before.dirty, "and it is still the owner's unsaved work");
}

#[test]
fn an_import_refuses_what_is_no_object_and_every_field_it_does_not_take() {
    let (mut s, _account) = session();
    let before = s.mailbox.clone();
    let html = document(&[PICTURE_FILE]);
    let good = asset(PICTURE_FILE, ONE_PIXEL_PNG);

    // No object at all, at the top and for an asset.
    for (label, params) in [
        ("null", json!(null)),
        ("a list", json!([])),
        ("a string", json!("name")),
        ("a number", json!(7)),
        ("an asset of no object", json!({"name": "Northwind", "html": html, "assets": ["studio-mark.png"]})),
    ] {
        let said = refused(s.run("signature.import", &params), label);
        assert!(is_bad_params(&said), "{label}: the refusal is the parameters': {said}");
    }

    // A field the command does not take, at the top and in an asset: never ignored, since `path`,
    // `default` and `account` would look as if they did something.
    for key in ["path", "default", "account", "file", "layout"] {
        let mut params = json!({"name": "Northwind", "html": html, "assets": [good.clone()]});
        params[key] = json!("junk");
        let said = refused(s.run("signature.import", &params), key);
        assert!(is_bad_params(&said), "`{key}`: the refusal is the parameters': {said}");
        assert!(said.to_string().contains(key), "`{key}`: the refusal names the field: {said}");
        assert!(!said.to_string().contains("junk"), "`{key}`: the refusal says nothing of the field's value: {said}");
    }
    for key in ["path", "mime", "size"] {
        let mut one = good.clone();
        one[key] = json!("junk");
        let said = refused(s.run("signature.import", &json!({"name": "Northwind", "html": html, "assets": [one]})), key);
        assert!(is_bad_params(&said), "an asset's `{key}`: the refusal is the parameters': {said}");
        assert!(said.to_string().contains(key), "an asset's `{key}`: the refusal names the field: {said}");
    }
    assert!(s.mailbox == before, "no refused import left anything in the mailbox");
    assert!(s.mailbox.signature("Northwind").is_none(), "no signature was added by a refusal");
}

#[test]
fn an_import_needs_its_fields_and_the_types_they_have() {
    let (mut s, _account) = session();
    let before = s.mailbox.clone();
    let html = document(&[PICTURE_FILE]);
    let good = asset(PICTURE_FILE, ONE_PIXEL_PNG);

    let cases = [
        ("no name", json!({"html": html, "assets": [good.clone()]})),
        ("no html", json!({"name": "Northwind", "assets": [good.clone()]})),
        ("no assets", json!({"name": "Northwind", "html": html})),
        ("a name of no string", json!({"name": 7, "html": html, "assets": [good.clone()]})),
        ("an empty name", json!({"name": " \r\n ", "html": html, "assets": [good.clone()]})),
        ("an html of no string", json!({"name": "Northwind", "html": [html], "assets": [good.clone()]})),
        ("assets of no list", json!({"name": "Northwind", "html": html, "assets": {"0": good.clone()}})),
        ("assets of a string", json!({"name": "Northwind", "html": html, "assets": "studio-mark.png"})),
        ("an asset of no name", json!({"name": "Northwind", "html": html, "assets": [{"data": good["data"].clone()}]})),
        ("an asset of no data", json!({"name": "Northwind", "html": html, "assets": [{"name": PICTURE_FILE}]})),
        ("a data of no string", json!({"name": "Northwind", "html": html, "assets": [{"name": PICTURE_FILE, "data": 5}]})),
    ];
    for (label, params) in cases {
        let said = refused(s.run("signature.import", &params), label);
        assert!(is_bad_params(&said), "{label}: the refusal is the parameters': {said}");
    }
    assert!(s.mailbox == before, "no refused import changed the mailbox");
}

#[test]
fn an_import_refuses_data_that_is_no_base64() {
    let (mut s, _account) = session();
    let before = s.mailbox.clone();

    let said = refused(
        s.run(
            "signature.import",
            &json!({"name": "Northwind", "html": document(&[PICTURE_FILE]), "assets": [{"name": PICTURE_FILE, "data": "!!!!"}]}),
        ),
        "an asset whose data is no base64",
    );

    assert!(is_bad_params(&said), "the refusal is the parameters': {said}");
    assert!(said.to_string().contains("`data` must be base64"), "the refusal says what is wrong with `data`: {said}");
    assert!(s.mailbox == before, "the refused import changed nothing");
}

#[test]
fn a_picture_exactly_at_its_byte_cap_is_taken_with_the_line_breaks_of_a_mail_writer() {
    let (mut s, _account) = session();
    let big = picture_of(MAX_PICTURE_BYTES);
    let encoded = mime_base64(&big);
    assert!(encoded.contains("\r\n"), "the fixture's data is broken into lines, as a mail writer breaks it");
    assert!(encoded.chars().filter(|c| !c.is_whitespace()).count() <= MAX_ENCODED, "the fixture is within a picture's base64");

    let answer = run(
        &mut s,
        "signature.import",
        json!({"name": "Northwind", "html": document(&[PICTURE_FILE]), "assets": [{"name": PICTURE_FILE, "data": encoded}]}),
    );
    assert_eq!(answer, json!({"name": "Northwind"}));
    let stored = s.mailbox.signature("Northwind").expect("the picture at its cap was taken");
    assert_eq!(stored.pictures().first().map(|p| p.data.len()), Some(MAX_PICTURE_BYTES), "the picture's own bytes are kept whole, at the cap");
}

#[test]
fn a_picture_one_byte_over_its_cap_is_refused_and_leaves_the_mailbox_as_it_was() {
    let (mut s, _account) = session();
    let before = s.mailbox.clone();
    let over = picture_of(MAX_PICTURE_BYTES + 1);

    let said = refused(
        s.run("signature.import", &json!({"name": "Northwind", "html": document(&[PICTURE_FILE]), "assets": [asset(PICTURE_FILE, &over)]})),
        "a picture one byte over its cap",
    );

    assert!(is_bad_params(&said), "the refusal is the parameters': {said}");
    assert!(said.to_string().contains("larger than a signature's picture"), "the refusal says which cap: {said}");
    assert!(s.mailbox == before, "the mailbox is as it was");
    assert!(s.mailbox.signature("Northwind").is_none(), "and no signature was made of it");
}

#[test]
fn an_import_refuses_a_picture_file_over_its_caps_before_anything_is_decoded() {
    let (mut s, _account) = session();
    let before = s.mailbox.clone();
    let html = document(&[PICTURE_FILE]);

    // A name longer than a picture's file name may be.
    let said = refused(
        s.run(
            "signature.import",
            &json!({"name": "Northwind", "html": html, "assets": [{"name": "n".repeat(MAX_ASSET_NAME + 1), "data": base64_encode(ONE_PIXEL_PNG)}]}),
        ),
        "a picture file's name over its cap",
    );
    assert!(is_bad_params(&said), "the refusal is the parameters': {said}");
    assert!(said.to_string().contains("file name is longer"), "the refusal says which cap: {said}");

    // More base64 characters than one picture decodes from: counted, never decoded.
    let said = refused(
        s.run(
            "signature.import",
            &json!({"name": "Northwind", "html": html, "assets": [{"name": PICTURE_FILE, "data": "A".repeat(MAX_ENCODED + 1)}]}),
        ),
        "a picture's data over the characters a picture decodes from",
    );
    assert!(is_bad_params(&said), "the refusal is the parameters': {said}");
    assert!(said.to_string().contains("`data` is longer"), "the refusal is the encoded length's, not the decoder's: {said}");

    // A `data` string longer than the room its line breaks may take.
    let said = refused(
        s.run(
            "signature.import",
            &json!({"name": "Northwind", "html": html, "assets": [{"name": PICTURE_FILE, "data": " ".repeat(MAX_ENCODED * 4 + 1)}]}),
        ),
        "a picture's data string over its byte budget",
    );
    assert!(is_bad_params(&said), "the refusal is the parameters': {said}");
    assert!(said.to_string().contains("`data` is longer"), "the refusal is the raw length's: {said}");

    assert!(s.mailbox == before, "no refused import changed the mailbox");
}

#[test]
fn pictures_larger_together_than_a_signature_holds_are_refused() {
    let (mut s, _account) = session();
    let before = s.mailbox.clone();
    let each = MAX_PICTURES_BYTES / 3 + 1;
    assert!(each <= MAX_PICTURE_BYTES, "each picture is within its own cap, so the total is what refuses");
    let files = ["a.png", "b.png", "c.png"];
    let assets: Vec<Value> = files.iter().map(|f| asset(f, &picture_of(each))).collect();
    assert!(each * 3 > MAX_PICTURES_BYTES, "the fixture's pictures are over the total they may have together");

    let said = refused(
        s.run("signature.import", &json!({"name": "Northwind", "html": document(&files), "assets": assets})),
        "pictures over the total they may have together",
    );

    assert!(is_bad_params(&said), "the refusal is the parameters': {said}");
    assert!(said.to_string().contains("larger than a signature's pictures"), "the refusal says which cap: {said}");
    assert!(s.mailbox == before, "the refused import changed nothing");
}

#[test]
fn more_picture_files_than_a_signature_holds_are_refused() {
    let (mut s, _account) = session();
    let before = s.mailbox.clone();
    let assets: Vec<Value> = (0..MAX_PICTURES + 1).map(|n| asset(&format!("mark-{n}.png"), ONE_PIXEL_PNG)).collect();

    let said = refused(
        s.run("signature.import", &json!({"name": "Northwind", "html": document(&[PICTURE_FILE]), "assets": assets})),
        "more picture files than a signature holds",
    );

    assert!(is_bad_params(&said), "the refusal is the parameters': {said}");
    assert!(said.to_string().contains("more picture files"), "the refusal says which cap: {said}");
    assert!(s.mailbox == before, "the refused import changed nothing");
}

#[test]
fn a_document_longer_than_a_signature_may_have_is_refused() {
    let (mut s, _account) = session();
    let before = s.mailbox.clone();
    let long = format!("<div>{}</div>", "x".repeat(MAX_HTML));

    let said =
        refused(s.run("signature.import", &json!({"name": "Northwind", "html": long, "assets": []})), "a document over a signature's HTML cap");

    assert!(is_bad_params(&said), "the refusal is the parameters': {said}");
    assert!(said.to_string().contains("`html` is longer"), "the refusal says which cap: {said}");
    assert!(s.mailbox == before, "the refused import changed nothing");
}

#[test]
fn an_import_naming_a_picture_it_was_not_given_is_refused() {
    let (mut s, _account) = session();
    let before = s.mailbox.clone();

    let said = refused(
        s.run(
            "signature.import",
            &json!({"name": "Northwind", "html": document(&[PICTURE_FILE]), "assets": [asset("other-mark.png", ONE_PIXEL_PNG)]}),
        ),
        "an import naming a picture it was not given",
    );

    assert!(!matches!(said, CmdError::Unknown(_) | CmdError::Params(_)), "the document's own refusal, not the door's: {said}");
    assert!(
        said.to_string().contains("a picture's address names no supplied file"),
        "the refusal names the missing picture file, in the importer's own words: {said}"
    );
    assert!(s.mailbox == before, "the refused import left the mailbox as it was");
}

#[test]
fn no_import_goes_into_a_mailbox_that_is_full() {
    let (mut s, _account) = session();
    // Filled by the existing command, so that it is the command's own cap this meets, and no
    // larger a fixture than a mailbox may hold.
    let mut added = 0u32;
    loop {
        let before = s.mailbox.signatures.len();
        let name = format!("Import Filler {added}");
        if s.run("signature.set", &json!({"name": name, "text": "Filler"})).is_err() {
            break;
        }
        assert_eq!(s.mailbox.signatures.len(), before + 1, "a filler added a signature");
        added += 1;
        assert!(added < 200, "the mailbox took far more signatures than a mailbox may hold");
    }
    assert_eq!(s.mailbox.signatures.len(), 100, "a mailbox holds the model's hundred signatures");
    let before = s.mailbox.clone();

    let said = refused(
        s.run("signature.import", &json!({"name": "Northwind", "html": document(&[PICTURE_FILE]), "assets": [asset(PICTURE_FILE, ONE_PIXEL_PNG)]})),
        "an import into a full mailbox",
    );

    assert!(!matches!(said, CmdError::Unknown(_)), "the command is there, and refused for its own reason: {said}");
    assert!(said.to_string().contains("too many signatures"), "the refusal says the mailbox is full: {said}");
    assert!(s.mailbox == before, "the full mailbox was not grown");
    assert!(s.mailbox.signature("Northwind").is_none(), "and no signature was added");
}
