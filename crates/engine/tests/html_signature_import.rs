// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An HTML signature imported by the command the app runs, followed from the import to the draft
//! the mailbox keeps and to the message as it would go on the wire: the owner's nested tables and
//! inline styling are there, the words are there, and every picture the HTML names is an inline
//! part of that same message under the Content-ID the HTML names there, with the picture's own
//! bytes — named in two places, one part. The stored signature is the owner's document byte for
//! byte, but for each picture's address, which is written as the address the signature holds that
//! picture under.
//!
//! The document is one this version keeps as it is (inline styling and tables, no style sheet):
//! that is what `signature.import` is for. A second tracer asks for a picture that wasn't
//! supplied and holds the refusal to the whole mailbox and to the signature open in the editor,
//! unchanged.
//!
//! The people, hosts and picture are invented: the picture is the synthetic one-pixel PNG the
//! MIME tests use, and says nothing of pixels. Nothing here reads a file, opens a dialog or sends
//! anything.

use mailcraft_engine::cmd::base64_encode;
use mailcraft_engine::{CmdError, Session, sample};
use mailcraft_mime_model::WriteOptions;
use mailcraft_mime_model::rich::{ImgRef, from_html_with_pictures, internal_cid};
use mailcraft_model::FolderKind;
use mailcraft_model::signature::Layout;
use mailcraft_time::{Date, DateTime};
use serde_json::{Value, json};

/// A one-pixel PNG, whole and valid: a picture by its own first bytes, the same fixture the MIME
/// integration tests use. Nothing here says anything about pixels.
const ONE_PIXEL_PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,
    0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0a, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

/// The picture's file name beside the owner's document: the name the import is given, and the one
/// the document's `<img>` names.
const PICTURE_FILE: &str = "studio-mark.png";

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

/// The owner's own document: two tables, one inside the other, their look in `style` attributes,
/// and the picture named by its file name in two places — one this version keeps as it is.
fn own_html() -> &'static str {
    r#"<table width="520" cellpadding="0" cellspacing="0" border="0" style="width:520px;background-color:#fdf6e3;padding:12px;font-family:Georgia,serif"><tr>
  <td style="padding-right:16px"><img src="studio-mark.png" alt="Northwind mark" width="120" height="80" style="border:0"></td>
  <td style="font-family:Georgia,serif"><table cellpadding="0" cellspacing="0" border="0" style="background-color:#ffffff;padding:8px"><tr>
    <td><div>Priya Raman - Northwind Atelier</div><div>Studio Two, Harbour Lane</div><div><a href="mailto:hello@example.org">hello@example.org</a></div></td>
  </tr></table></td>
</tr></table>
<div><img src="studio-mark.png" alt="Northwind mark" width="120" height="80" style="border:0"></div>"#
}

/// Every picture `html` names by a `cid:` address, as the real HTML reader of a draft of this
/// program finds them: by what it reads of each `<img>`, not by a search of the text.
fn image_cids(html: &str) -> Vec<String> {
    let mut cids = Vec::new();
    let mut claim = |img: &ImgRef| {
        cids.push(img.cid.clone());
        Some(0)
    };
    let _ = from_html_with_pictures(html, &mut claim);
    cids
}

/// Every picture `html` names is an inline part of the same message, under the Content-ID it names
/// there, with the picture's own bytes; no address the signature keeps for itself is left in the
/// HTML, and no inline picture part is left unnamed.
fn assert_pictures_bound(label: &str, html: &str, parts: &[(String, Vec<u8>)]) {
    let cids = image_cids(html);
    assert_eq!(cids.len(), 2, "{label}: the HTML names the signature's one picture in both places it is written: {cids:?}");
    assert!(!cids.is_empty(), "{label}: the HTML names no picture at all: {html}");
    assert!(
        parts.iter().any(|(_, bytes)| bytes.as_slice() == ONE_PIXEL_PNG),
        "{label}: the signature's picture is no inline part with its own bytes: {parts:?}"
    );
    for cid in &cids {
        assert!(
            !cid.contains("signature.invalid"),
            "{label}: the HTML keeps the signature's own address `{cid}` instead of a Content-ID of this message; its inline parts are {parts:?}: {html}"
        );
        let part = parts
            .iter()
            .find(|(id, _)| id == cid)
            .unwrap_or_else(|| panic!("{label}: `{cid}` is no Content-ID of an inline part of this message: {parts:?}"));
        assert_eq!(part.1.as_slice(), ONE_PIXEL_PNG, "{label}: the inline part `{cid}` is not the picture's own bytes");
    }
    for (id, _) in parts {
        assert!(cids.contains(id), "{label}: the inline part `{id}` is named by no picture of the HTML: {html}");
    }
}

#[test]
fn an_imported_html_signature_goes_into_a_saved_draft_with_its_pictures_bound() {
    let now = DateTime::new(Date::new(2026, 10, 14).unwrap(), 16, 0, 0).unwrap().to_timestamp(0);
    let mut s = Session::at(sample::mailbox(now, 0), now, 0);
    let account = s.mailbox.default_account().expect("the sample mailbox has an account").id;
    let default_before = s.mailbox.account(account).expect("the account just found").signature.clone();

    // The owner's document and its picture file, as the command is given them: nothing of a path,
    // of a default, of an account, or of what it should look like.
    let own = own_html();
    let answer = run(
        &mut s,
        "signature.import",
        json!({
            "name": " Northwind Studio ",
            "html": own,
            "assets": [{"name": PICTURE_FILE, "data": base64_encode(ONE_PIXEL_PNG)}],
        }),
    );
    // Its name cleaned, and nothing else: no editor opened, no message started, no default set.
    assert_eq!(answer, json!({"name": "Northwind Studio"}), "the import answers the signature's clean name and nothing else");
    assert!(s.signature_edit.is_none(), "the import opened no signature in the editor");
    assert!(s.compose.is_none(), "the import opened nothing to write");
    assert_eq!(
        s.mailbox.account(account).expect("the account").signature,
        default_before,
        "the account's default signature is untouched by the import"
    );

    // What the mailbox kept of it: the owner's document byte for byte, but for the picture's
    // address, and the picture with its own name and bytes.
    let stored = s.mailbox.signature("Northwind Studio").expect("the import put the signature in the mailbox");
    assert_eq!(stored.layout, Layout::Inline, "an imported document lays its pictures out in its own tables");
    assert_eq!(stored.pictures().len(), 1, "one picture for the two places the document names it: {:?}", stored.pictures());
    let picture = stored.pictures().first().expect("the picture just counted");
    assert_eq!(picture.name, PICTURE_FILE, "the picture is named by its file's own name");
    assert_eq!(picture.data.as_slice(), ONE_PIXEL_PNG, "the picture's bytes are kept as they are");
    let expected = own.replace(&format!("src=\"{PICTURE_FILE}\""), &format!("src=\"cid:{}\"", internal_cid(picture)));
    assert_eq!(stored.html(), Some(expected.as_str()), "the document is kept as written, only the picture's address changed");
    let stand_in = stored.body().text();
    assert!(stand_in.contains("Priya Raman - Northwind Atelier"), "the text beside the HTML keeps the owner's words: {stand_in:?}");
    assert!(stand_in.contains("Studio Two, Harbour Lane"), "and the rest of the address: {stand_in:?}");

    // The account's signature for new messages, then a message saved as a draft by the command the
    // app's Save Draft runs.
    run(&mut s, "signature.defaults", json!({"account": account, "new": "Northwind Studio"}));
    run(&mut s, "mail.new", json!({"account": account, "body": "Thanks,"}));
    let saved = run(&mut s, "compose.saveDraft", json!({}))["draft"].as_u64().expect("the saved draft's id");

    // What the mailbox kept of that, read back by its id: not what the test put in.
    let draft = s.mailbox.message(saved).expect("the saved draft is in the mailbox");
    assert!(draft.draft, "saved as a draft");
    assert_eq!(s.mailbox.folder(draft.folder).map(|f| f.kind), Some(FolderKind::Drafts), "saved in Drafts");
    assert!(draft.body.text.contains("Thanks,"), "the owner's words are kept: {:?}", draft.body.text);
    assert!(draft.body.text.contains("Priya Raman - Northwind Atelier"), "the signature's words are kept: {:?}", draft.body.text);
    let html = draft.body.html.as_deref().expect("an HTML message keeps its HTML");

    // The owner's own tables, one inside the other, and the styling they carry inline.
    assert_eq!(html.matches("<table").count(), 2, "the owner's nested tables, and none of the helper's own: {html}");
    let css: String = html.chars().filter(|c| !c.is_whitespace()).collect();
    for kept in
        ["width:520px", "background-color:#fdf6e3", "padding:12px", "background-color:#ffffff", "padding-right:16px", "font-family:Georgia,serif"]
    {
        assert!(css.contains(kept), "the owner's `{kept}` kept in: {html}");
    }

    // The pictures of the draft: the signature's one, one inline part of it with its own bytes,
    // and both places the HTML names it are that part's Content-ID, in this message.
    let parts: Vec<(String, Vec<u8>)> =
        draft.attachments.iter().filter(|a| a.inline && a.mime == "image/png").map(|a| (a.content_id.clone(), a.data.to_vec())).collect();
    assert_eq!(parts.len(), 1, "the picture the signature names in two places is one inline part of the draft: {:?}", draft.attachments);
    assert_pictures_bound("the saved draft", html, &parts);

    // And it is the binding a draft is written as, too: the same addresses on the wire.
    let wire = mailcraft_mime_model::write(draft, &WriteOptions { include_bcc: true, ..Default::default() });
    let parsed = mailcraft_mime_model::parse(&wire);
    let wire_html = parsed.body.html.as_deref().expect("the written draft keeps its HTML");
    let wire_parts: Vec<(String, Vec<u8>)> =
        parsed.attachments.iter().filter(|a| a.inline && a.mime == "image/png").map(|a| (a.content_id.clone(), a.data.to_vec())).collect();
    assert_eq!(wire_parts.len(), 1, "the picture written as an inline part: {:?}", parsed.attachments);
    assert_pictures_bound("the written draft", wire_html, &wire_parts);
}

#[test]
fn an_import_naming_a_picture_it_was_not_given_is_refused_and_leaves_everything_as_it_was() {
    let now = DateTime::new(Date::new(2026, 10, 14).unwrap(), 16, 0, 0).unwrap().to_timestamp(0);
    let mut s = Session::at(sample::mailbox(now, 0), now, 0);
    let account = s.mailbox.default_account().expect("the sample mailbox has an account").id;

    // Existing commands only: two signatures, the account's default, then one of them open in the
    // editor with words typed into it that aren't saved.
    run(&mut s, "signature.set", json!({"name": "Standard", "text": "Priya Raman\nNorthwind Atelier"}));
    run(&mut s, "signature.set", json!({"name": "Studio", "text": "Priya Raman\nStudio Two, Harbour Lane"}));
    run(&mut s, "signature.defaults", json!({"account": account, "new": "Standard"}));
    run(&mut s, "signature.open", json!({"name": "Studio"}));
    run(&mut s, "compose.type", json!({"text": "Unsaved words for the studio signature"}));

    let before_mailbox = s.mailbox.clone();
    let before_signatures = s.mailbox.signatures.len();
    let before_defaults = {
        let a = s.mailbox.account(account).expect("the account");
        (a.signature.clone(), a.reply_signature().to_string())
    };
    let before_editor = s.signature_edit.clone().expect("a signature is open in the editor");
    assert!(before_editor.dirty, "the editor has changes that aren't saved");
    // The fixture put both the typed words and the signature's own words in the open editor
    // before the import is even run: the refusal below is judged on the words that were here.
    let before_typed = before_editor.body.text().text();
    assert!(
        before_typed.contains("Unsaved words for the studio signature"),
        "the unsaved words were typed into the open signature: {before_typed:?}"
    );
    assert!(before_typed.contains("Priya Raman"), "the signature's own words are in the open editor: {before_typed:?}");

    // The document names `studio-mark.png`; the only file supplied is another one.
    let refused = s.run(
        "signature.import",
        &json!({
            "name": "Northwind Studio",
            "html": own_html(),
            "assets": [{"name": "other-mark.png", "data": base64_encode(ONE_PIXEL_PNG)}],
        }),
    );
    let said = match refused {
        Err(e) => e,
        Ok(answer) => panic!("the import named a picture it was not given, and answered {answer}"),
    };
    assert!(!matches!(&said, CmdError::Unknown(_)), "the command is there, and refused for its own reason: {said}");
    let said = said.to_string();
    assert!(
        said.contains("a picture's address names no supplied file"),
        "the refusal names the picture file it is missing, not merely that something failed: {said}"
    );

    // Nothing of the mailbox changed: not the signatures, not the account's defaults.
    assert!(s.mailbox == before_mailbox, "the refused import left the mailbox as it was");
    assert_eq!(s.mailbox.signatures.len(), before_signatures, "no signature was added");
    assert!(s.mailbox.signature("Northwind Studio").is_none(), "the refused import added no signature of that name");
    let after_defaults = {
        let a = s.mailbox.account(account).expect("the account");
        (a.signature.clone(), a.reply_signature().to_string())
    };
    assert_eq!(after_defaults, before_defaults, "the account's default signatures are as they were");

    // Nor the signature open in the editor: its identity, its own content, its words with the
    // unsaved addition, its selection and its unsaved state.
    let after_editor = s.signature_edit.clone().expect("the signature is still open");
    assert_eq!(after_editor, before_editor, "the open signature, its unsaved text and its selection are as they were");
    assert_eq!(after_editor.signature.name, before_editor.signature.name, "the same signature is open in the editor");
    assert_eq!(
        s.signature_edit.as_ref().map(|e| e.signature.name.as_str()),
        Some(before_editor.signature.name.as_str()),
        "the session names the same signature"
    );
    assert_eq!(after_editor.signature, before_editor.signature, "the signature's own content is as it was");
    assert_eq!(after_editor.body.selection(), before_editor.body.selection(), "the selection is as it was");
    assert_eq!(after_editor.body.text(), before_editor.body.text(), "the editor's whole text is as it was");
    let typed = after_editor.body.text().text();
    assert!(typed.contains("Unsaved words for the studio signature"), "the unsaved words are still there: {typed:?}");
    assert!(typed.contains("Priya Raman"), "and so is what the signature had: {typed:?}");
    assert!(after_editor.dirty, "the editor still has its changes");
    assert!(s.compose.is_none(), "no message was opened by the refusal");
}
