// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An authored signature's style sheet written out scoped, beside a received message's quote,
//! which stays as it was:
//!
//! * a tracer: a signature with a class rule in its `<style>`, a table and one picture named
//!   twice, written by `authored_fragment_with_rebound_cids` under a scope class, then put in a
//!   whole message after an owner paragraph that has the same class: the display reader gives the
//!   signature's words the rule's look and the owner's words none, the rule's declarations go in
//!   the head as written, the body holds no style sheet, and both pictures name the bound address;
//! * a control: `quotable` and `quotable_with_rebound_cids` write what the existing tests say.
//!
//! The people, addresses, rule and pictures are invented. Nothing here reads a file, fetches or
//! sends.

use mailcraft_html::{
    AuthoredScope, Block, BlockKind, Rgb, Span, authored_fragment_with_rebound_cids, image_sources, quotable, quotable_with_rebound_cids,
};

/// The Content-ID the signature's picture has where it is kept, and the one it goes out under.
const INTERNAL_CID: &str = "studio-mark.7f3a@internal.invalid";
const BOUND_CID: &str = "bound@signature.invalid";
/// The scope class the signature's rules are written under.
const SCOPE: &str = "mc-sig-test";
/// The words the class rule styles, and what that rule gives them.
const SIGNATURE_WORDS: &str = "Northwind Studio card";
const CARD_COLOR: Rgb = Rgb(31, 78, 121);
/// The rule's declaration block, as the owner wrote it.
const RULE_BLOCK: &str = "{ color: #1f4e79; font-weight: bold; padding: 13px; }";
/// The owner's words in the message, outside the signature, under the same class.
const OWNER_WORDS: &str = "Lakeside owner reply about Thursday";
const OWNER_PARAGRAPH: &str = r#"<p class="studio-card">Lakeside owner reply about Thursday</p>"#;
const SHEET: &str = "<style>.studio-card { color: #1f4e79; font-weight: bold; padding: 13px; }</style>";
/// The signature as authored: its style sheet in the head, a table holding the marked words, and
/// the one picture named twice by its internal Content-ID.
const SIGNATURE: &str = r#"<head><style>.studio-card { color: #1f4e79; font-weight: bold; padding: 13px; }</style></head><body>
<table cellpadding="0" cellspacing="0" border="0"><tr>
<td><img src="cid:studio-mark.7f3a@internal.invalid" alt="Northwind mark" width="120" height="80"></td>
<td><div class="studio-card">Northwind Studio card</div></td>
<td><img src="cid:studio-mark.7f3a@internal.invalid" alt="Northwind mark again" width="120" height="80"></td>
</tr></table></body>"#;

/// The span the display reader makes of `text`, looking inside tables (the reading of
/// `crates/engine/tests/html_signature_fidelity.rs`).
fn span_of(blocks: &[Block], text: &str) -> Option<Span> {
    blocks.iter().find_map(|b| match &b.kind {
        BlockKind::Table(t) => t.rows.iter().flat_map(|r| &r.cells).find_map(|c| span_of(&c.blocks, text)),
        _ => b.spans.iter().find(|s| s.text.contains(text)).cloned(),
    })
}

/// Whether `text` is in a cell of a table among `blocks`.
fn in_a_table(blocks: &[Block], text: &str) -> bool {
    blocks.iter().any(|b| matches!(&b.kind, BlockKind::Table(t) if t.rows.iter().flat_map(|r| &r.cells).any(|c| span_of(&c.blocks, text).is_some())))
}

/// Tracer. Surface RED until the authored writer exists: this target does not compile.
#[test]
fn an_authored_signature_keeps_its_class_rule_scoped_to_itself_and_its_pictures_bound() {
    // Fixture guards: the display reader gives the marked words the rule's look only because of
    // the sheet, the unscoped sheet would give the owner's words that look too, and the real
    // locator finds the one internal address twice.
    let styled = span_of(&mailcraft_html::parse(SIGNATURE), SIGNATURE_WORDS).expect("the display reader reads the signature's words");
    assert!(styled.bold && styled.color == Some(CARD_COLOR), "the class rule styles the signature's words: {styled:?}");
    let leaked = format!("<html><head>{SHEET}</head><body>{OWNER_PARAGRAPH}</body></html>");
    let leaked = span_of(&mailcraft_html::parse(&leaked), OWNER_WORDS).expect("the owner's words are read");
    assert!(leaked.bold && leaked.color == Some(CARD_COLOR), "an unscoped rule would style the owner's words: {leaked:?}");
    let sources = image_sources(SIGNATURE).expect("the locator reads the signature");
    assert_eq!(sources.iter().map(|s| s.src.as_str()).collect::<Vec<_>>(), [format!("cid:{INTERNAL_CID}"), format!("cid:{INTERNAL_CID}")]);

    let scope = AuthoredScope::new(SCOPE).expect("a valid scope class");
    let mut rebind = |cid: &str| match cid {
        INTERNAL_CID => Some(BOUND_CID.to_string()),
        _ => None,
    };
    let got = authored_fragment_with_rebound_cids(SIGNATURE, &scope, &mut rebind).expect("the signature is written");

    // The rule goes in the head with its declarations as written; the body holds no style sheet.
    assert!(got.head.contains(RULE_BLOCK), "the rule's declaration block, verbatim: {}", got.head);
    assert!(!got.body.to_ascii_lowercase().contains("<style"), "no style sheet in the body: {}", got.body);

    // Both pictures name the bound address, and the internal one is gone.
    let sources = image_sources(&got.body).expect("the locator reads the written body");
    let bound: Vec<&str> = sources.iter().map(|s| s.src.as_str()).collect();
    assert_eq!(bound, [format!("cid:{BOUND_CID}"), format!("cid:{BOUND_CID}")], "both pictures bound: {}", got.body);
    assert!(!got.body.contains(INTERNAL_CID) && !got.head.contains(INTERNAL_CID), "no internal address left: {} {}", got.head, got.body);

    // In a whole message after the owner's own paragraph of the same class.
    let message = format!("<html><head>{}</head><body>{OWNER_PARAGRAPH}{}</body></html>", got.head, got.body);
    let blocks = mailcraft_html::parse(&message);
    let signature = span_of(&blocks, SIGNATURE_WORDS).unwrap_or_else(|| panic!("the signature's words are read: {message}"));
    assert!(signature.bold && signature.color == Some(CARD_COLOR), "the signature's words keep the rule's look: {signature:?} in {message}");
    let owner = span_of(&blocks, OWNER_WORDS).unwrap_or_else(|| panic!("the owner's words are read: {message}"));
    assert!(!owner.bold && owner.color != Some(CARD_COLOR), "the owner's words take nothing of the signature's rule: {owner:?} in {message}");

    // The table is kept.
    assert_eq!(got.body.matches("<table").count(), 1, "one table: {}", got.body);
    assert!(in_a_table(&blocks, SIGNATURE_WORDS), "the signature's words are still in its table: {message}");
}

/// Control: a received message is quoted as before. Every input and expected output below is
/// one the existing tests already assert, cited at each.
#[test]
fn a_received_message_is_quoted_as_the_existing_tests_say() {
    // crates/html/src/quote/tests.rs:54-57 (`only_the_body_goes`).
    let html = "<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>T</title><style>.a{color:red}</style>\
                <link rel=stylesheet href=https://evil.example/x.css><base href=https://evil.example/></head>\
                <body bgcolor=#fff onload=\"x()\"><p>Hello</p></body></html>";
    assert_eq!(quotable(html), "<p>Hello</p>");
    // crates/html/src/quote/tests.rs:69 with :83 (`what_runs_or_embeds_goes_with_its_content`).
    assert_eq!(quotable("a<style>p{color:red}</style>b"), "ab");
    // crates/html/src/quote_cid_tests.rs:11-15 with :33-34 (`the_scheme_is_read_in_any_letter_case`): the
    // map's one entry these inputs reach.
    let mut rebind = |cid: &str| match cid {
        "logo@example.org" => Some("part-1@mailcraft.invalid".to_string()),
        _ => None,
    };
    for html in ["<img src=\"CID:logo@example.org\">", "<img src=\" cid:logo@example.org \">"] {
        assert_eq!(quotable_with_rebound_cids(html, &mut rebind), "<img src=\"cid:part-1@mailcraft.invalid\">", "{html}");
    }
}

/// What the authored writer gives `html` under the tracer's scope, rebinding no picture.
fn written(html: &str) -> Result<mailcraft_html::AuthoredFragment, mailcraft_html::AuthoredError> {
    let scope = AuthoredScope::new(SCOPE).expect("a valid scope class");
    authored_fragment_with_rebound_cids(html, &scope, &mut |_: &str| None::<String>)
}

/// An `@media` block keeps its prelude as written, with the rules inside it scoped.
#[test]
fn a_media_block_is_kept_as_written_round_its_scoped_rules() {
    let html = r#"<style>@media screen and (max-width: 600px) { .narrow { color: #102030; } }</style><div class="narrow">Narrow words</div>"#;
    let got = written(html).expect("the signature is written");
    let media = got.head.find("@media screen and (max-width: 600px) {").unwrap_or_else(|| panic!("the media block as written: {}", got.head));
    let rule = ".mc-sig-test .narrow, .mc-sig-test.narrow { color: #102030; }";
    let at = got.head.find(rule).unwrap_or_else(|| panic!("the scoped rule: {}", got.head));
    assert!(media < at && got.head[at + rule.len()..].contains('}'), "the rule inside the media block: {}", got.head);
    assert!(got.body.contains("Narrow words"), "{}", got.body);
}

/// A class rule could have matched the signature's own `<body>`: it is written for the wrapper,
/// which carries that body's class, as well as inside it.
#[test]
fn a_tagless_leftmost_selector_is_scoped_inside_and_on_the_wrapper() {
    let html = r#"<head><style>.c { font-weight: bold; }</style></head><body class="c"><p>Body words</p></body>"#;
    let got = written(html).expect("the signature is written");
    assert!(got.head.contains(".mc-sig-test .c, .mc-sig-test.c { font-weight: bold; }"), "{}", got.head);
    assert!(got.body.starts_with(r#"<div class="mc-sig-test c">"#), "{}", got.body);
    let message = format!("<html><head>{}</head><body>{}</body></html>", got.head, got.body);
    let words = span_of(&mailcraft_html::parse(&message), "Body words").unwrap_or_else(|| panic!("read: {message}"));
    assert!(words.bold, "the body's class rule still holds: {words:?} in {message}");
}

/// `@import` would fetch a sheet in the recipient's program.
#[test]
fn an_import_is_unsupported() {
    let html = r#"<style>@import "https://example.org/x.css"; .c { color: #aa0000; }</style><p class="c">x</p>"#;
    assert_eq!(written(html), Err(mailcraft_html::AuthoredError::Unsupported));
}

/// A `url(` written with an escape is still one to a CSS reader.
#[test]
fn an_escaped_url_in_a_declaration_is_risky() {
    let html = r#"<style>.c { background: u\72l(https://example.org/t.png); }</style><p class="c">x</p>"#;
    assert_eq!(written(html), Err(mailcraft_html::AuthoredError::Risky));
}

/// `:is()` is a selector the reader drops: it is not written either.
#[test]
fn an_is_selector_is_unsupported() {
    let html = r#"<style>:is(.a, .b) { color: #aa0000; }</style><p class="a">x</p>"#;
    assert_eq!(written(html), Err(mailcraft_html::AuthoredError::Unsupported));
}

/// `:root` is the recipient's whole message, never the signature.
#[test]
fn a_root_selector_is_unsupported() {
    assert_eq!(written("<style>:root { color: #aa0000; }</style><p>x</p>"), Err(mailcraft_html::AuthoredError::Unsupported));
}

/// A `<style>` the input ends inside is no sheet as written.
#[test]
fn an_unclosed_style_is_refused() {
    assert_eq!(written("<p>x</p><style>.c { color: #aa0000; }"), Err(mailcraft_html::AuthoredError::Unclosed));
}

/// A scope class the signature already uses would scope nothing apart.
#[test]
fn a_scope_already_in_the_signature_collides() {
    assert_eq!(written(r#"<p class="mc-sig-test">x</p>"#), Err(mailcraft_html::AuthoredError::ScopeCollides));
}

/// A copy for narrow windows that the signature's sheet hides at full width is kept in the
/// body (a quote leaves it out), with the rule that hides it.
#[test]
fn a_copy_its_own_sheet_hides_is_kept() {
    let html = r#"<head><style>.mobile { display: none; } @media (max-width: 480px) { .mobile { display: block; } .desk { display: none; } }</style></head><body><div class="desk">Desktop card</div><div class="mobile">Mobile card</div></body>"#;
    let quoted = quotable(html);
    assert!(quoted.contains("Desktop card") && !quoted.contains("Mobile card"), "a quote leaves the hidden copy out: {quoted}");
    let got = written(html).expect("the signature is written");
    assert!(got.body.contains("Desktop card") && got.body.contains("Mobile card"), "both copies: {}", got.body);
    assert!(got.head.contains(".mc-sig-test .mobile, .mc-sig-test.mobile { display: none; }"), "{}", got.head);
}

// Review blocker 1: a line break CSS reads (CR, FF; LF after preprocessing) or a NUL inside a
// quoted string of a selector or an `@media` prelude must not let text past the scope. Each
// case is refused by the writer and so reported by the checker as MC-CSS-001, through the
// public `compat::check`.

/// The writer refuses `html` and the checker reports MC-CSS-001 for it.
fn refused_and_flagged(html: &str) {
    let got = written(html);
    assert!(
        matches!(got, Err(mailcraft_html::AuthoredError::Risky | mailcraft_html::AuthoredError::Unsupported)),
        "the writer refuses it: {got:?} for {html:?}"
    );
    let found = mailcraft_html::compat::check(html).unwrap_or_else(|e| panic!("the checker reads it: {e:?} for {html:?}"));
    let ids: Vec<&str> = found.iter().map(|f| f.rule.id()).collect();
    assert!(ids.contains(&"MC-CSS-001"), "MC-CSS-001 is reported: {ids:?} for {html:?}");
}

/// A carriage return ends a CSS string; the reader read on past it into another rule.
#[test]
fn a_carriage_return_in_a_selector_string_is_refused() {
    refused_and_flagged(
        "<style>[x=\"a\r] x {} html { color: red; background: url(https://t.example/p.png) } p[y=\"] { color: blue }</style><p>Hi</p>",
    );
}

/// A form feed ends a CSS string the same way.
#[test]
fn a_form_feed_in_a_selector_string_is_refused() {
    refused_and_flagged(
        "<style>[x=\"a\x0c] x {} html { color: red; background: url(https://t.example/p.png) } p[y=\"] { color: blue }</style><p>Hi</p>",
    );
}

/// A NUL is replaced by the recipient's reader, never read as it is here.
#[test]
fn a_nul_in_a_selector_string_is_refused() {
    refused_and_flagged("<style>[x=\"a\0] x {} html { color: red } p[y=\"] { color: blue }</style><p>Hi</p>");
}

/// A `url(` inside a selector's attribute value is held to the same guard as a declaration.
#[test]
fn a_url_in_a_selector_attribute_value_is_refused() {
    refused_and_flagged("<style>p[title=\"url(https://t.example/p.png)\"] { color: blue }</style><p title=\"x\">Hi</p>");
}

/// A carriage return in a string of an `@media` prelude.
#[test]
fn a_carriage_return_in_a_media_prelude_is_refused() {
    refused_and_flagged("<style>@media screen, \"a\r{} html { color: red } @media x\" { .c { color: blue } }</style><p class=\"c\">Hi</p>");
}

/// A `url(` hidden in a string of an `@media` prelude.
#[test]
fn a_url_in_a_media_prelude_is_refused() {
    refused_and_flagged("<style>@media screen, \"url(https://t.example/p.png)\" { .c { color: blue } }</style><p class=\"c\">Hi</p>");
}

/// Control: a plain line feed in a selector string stays refused, as before.
#[test]
fn a_line_feed_in_a_selector_string_stays_refused() {
    refused_and_flagged("<style>[x=\"a\n] x {} html { color: red } p[y=\"] { color: blue }</style><p>Hi</p>");
}

// Review-02 blocker: a selector's or an `@media` prelude's text is echoed into the head, so it
// must be read the way a CSS reader reads it. A comment hides brackets from a bracket count but
// not from a CSS reader, an unclosed `(` or `[` in a declaration block keeps a CSS reader inside
// that block past the `}`, and the arguments of a pseudo-class that never matches are not read
// here at all. Together they put a rule outside the scope. Each case is refused by the writer and
// reported by the checker as MC-CSS-001.

/// The reviewer's case: an unclosed `(` in one block, then comment-hidden parentheses in the
/// next selector's `:hover()` and `:not()`, leave `html:not(.zz)` as a top-level rule.
#[test]
fn comment_hidden_parentheses_after_an_unclosed_paren_are_refused() {
    refused_and_flagged("<style>p { x: ( }\nq:hover(/*(((*/)) } html:not(.zz /*))*/) { color: red }</style><p>Hi</p>");
}

/// The same with square brackets and a pseudo-element's arguments.
#[test]
fn comment_hidden_brackets_after_an_unclosed_bracket_are_refused() {
    refused_and_flagged("<style>p { x: [ }\nq::before(/*[[[*/]] } body:not(.zz /*]]*/) { color: red }</style><p>Hi</p>");
}

/// The same with the unclosed `(` inside an `@media` block.
#[test]
fn comment_hidden_parentheses_after_a_media_block_are_refused() {
    refused_and_flagged("<style>@media screen { p { x: ( } }\nq:hover(/*(((*/)) } html:not(.zz /*))*/) { color: red }</style><p>Hi</p>");
}

/// A variant that would hide a reply's quoted original.
#[test]
fn a_rule_that_would_hide_the_quoted_original_is_refused() {
    refused_and_flagged("<style>p { x: ( }\nq:hover(/*(((*/)) } blockquote:not(.zz /*))*/) { display: none }</style><p>Hi</p>");
}

/// A variant that would hide everything, through `:focus()` on a class.
#[test]
fn a_rule_that_would_hide_everything_is_refused() {
    refused_and_flagged("<style>.a { x: ( }\n.b:focus(/*(((*/)) } *:not(.zz /*))*/) { visibility: hidden }</style><p class=\"a b\">Hi</p>");
}

/// Each half on its own: an unclosed `(` or `[` in a declaration block.
#[test]
fn an_unclosed_bracket_in_a_declaration_block_is_refused() {
    refused_and_flagged("<style>p { x: ( }\nq { color: blue }</style><p>Hi</p>");
    refused_and_flagged("<style>p { x: [ }\nq { color: blue }</style><p>Hi</p>");
    refused_and_flagged("<style>p { x: ( ] }</style><p>Hi</p>");
    refused_and_flagged("<style>@media screen { p { x: [ } }</style><p>Hi</p>");
}

/// Each half on its own: a comment in a selector or an `@media` prelude.
#[test]
fn a_comment_in_a_selector_or_media_prelude_is_refused() {
    refused_and_flagged("<style>p:not(.zz /*)*/) { color: blue }</style><p>Hi</p>");
    refused_and_flagged("<style>p /* note */ q { color: blue }</style><p>Hi</p>");
    refused_and_flagged("<style>@media screen /* wide */ { p { color: blue } }</style><p>Hi</p>");
}

/// Each half on its own: arguments to a pseudo-class or pseudo-element that never matches.
#[test]
fn arguments_on_a_never_matching_pseudo_class_are_refused() {
    refused_and_flagged("<style>q:hover(x) { color: blue }</style><p>Hi</p>");
    refused_and_flagged("<style>q::before(x) { color: blue }</style><p>Hi</p>");
}

/// Unbalanced or crossed brackets in an `@media` prelude.
#[test]
fn unbalanced_brackets_in_a_media_prelude_are_refused() {
    refused_and_flagged("<style>@media (max-width: 600px] { p { color: blue } }</style><p>Hi</p>");
}

/// Control: an ordinary signature sheet, with `:not()`, `:nth-child()`, attribute selectors
/// (one with brackets in a quoted value), `@media (max-width:600px)`, and comments between rules
/// and inside declaration blocks, is written as before, exactly.
#[test]
fn an_ordinary_signature_sheet_is_still_written_unchanged() {
    let html = concat!(
        "<style>\n/* brand card */\n",
        "p:not(.muted) { color: #1f4e79; /* house blue */ font-weight: bold; }\n",
        "a[href^=\"https://example.org\"] { text-decoration: none; }\n",
        "a[title=\"Call (24h) [desk]\"] { color: #333333; }\n",
        "td:nth-child(2n+1), td:nth-of-type(odd) { padding: 4px; }\n",
        "/* narrow windows */\n",
        "@media (max-width:600px) { td[lang|=en] { display: block; } }\n",
        "</style><table><tr><td><p>Ada Example</p></td></tr></table>"
    );
    let got = written(html).expect("the signature is written");
    assert_eq!(
        got.head,
        concat!(
            "<style>.mc-sig-test p:not(.muted) { color: #1f4e79; /* house blue */ font-weight: bold; }\n",
            ".mc-sig-test a[href^=\"https://example.org\"] { text-decoration: none; }\n",
            ".mc-sig-test a[title=\"Call (24h) [desk]\"] { color: #333333; }\n",
            ".mc-sig-test td:nth-child(2n+1), .mc-sig-test td:nth-of-type(odd) { padding: 4px; }\n",
            "@media (max-width:600px) {.mc-sig-test td[lang|=en] { display: block; }\n}</style>"
        )
    );
    assert_eq!(mailcraft_html::compat::check(html).expect("checked"), []);
}
