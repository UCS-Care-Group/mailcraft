// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What the writer of an authored signature writes of its style sheets is what a recipient's
//! reader takes from them, and what MailCraft's own reader (the previews) takes too: a `media`
//! condition written with a character reference keeps its meaning, a sheet whose `media` the
//! tokenizer couldn't keep (past the attributes it keeps) is refused rather than sent for every
//! window, an attribute test that the wrapper standing for the body can't pass is refused as one
//! of the body is, and a selector ending in a space that isn't CSS white space names the class it
//! names in CSS. A `media` made only of such spaces is a media type a CSS reader doesn't know: the
//! sheet is for no window, in MailCraft's reader (the pane, a reply's quote and the plain text
//! alike) and in what the writer writes, which keeps the condition. Of a `media` written twice, the
//! first counts, a second past the attributes kept and one in capitals included.
//!
//! The documents are invented; nothing here is fetched.

use mailcraft_html::compat::check;
use mailcraft_html::{
    AuthoredError, AuthoredFragment, AuthoredScope, Block, BlockKind, Env, Rgb, Span, authored_fragment_with_rebound_cids, parse_in, quotable,
    to_text,
};

fn write(html: &str) -> Result<AuthoredFragment, AuthoredError> {
    let scope = AuthoredScope::new("mc-test").expect("a scope class");
    authored_fragment_with_rebound_cids(html, &scope, &mut |cid: &str| Some(cid.to_string()))
}

fn rules(html: &str) -> Vec<&'static str> {
    check(html).expect("checked").iter().map(|f| f.rule.id()).collect()
}

fn span<'b>(blocks: &'b [Block], text: &str) -> Option<&'b Span> {
    blocks.iter().find_map(|b| match &b.kind {
        BlockKind::Table(t) => t.rows.iter().flat_map(|r| &r.cells).find_map(|c| span(&c.blocks, text)),
        _ => b.spans.iter().find(|s| s.text.contains(text)),
    })
}

/// The colour MailCraft's reader gives the words `text` of `html`.
fn colour(html: &str, text: &str) -> Option<Rgb> {
    span(&mailcraft_html::parse(html), text).unwrap_or_else(|| panic!("the reader reads {text:?} in {html}")).color
}

#[test]
fn a_media_condition_with_a_character_reference_is_written_as_it_reads() {
    let html = r#"<html><head><style media="screen and (min-width&#58; 600px)">.card { color: red }</style></head><body><p class="card">Priya Raman</p></body></html>"#;
    let written = write(html).expect("written");
    assert!(written.head.contains(r#"<style media="screen and (min-width: 600px)">"#), "the condition no longer says what it said: {}", written.head);
    assert!(!written.head.contains("&amp;"), "a character reference written twice: {}", written.head);
    // Control: the same condition written with its colour as it is.
    let plain = write(&html.replace("&#58;", ":")).expect("written");
    assert_eq!(plain.head, written.head, "the two ways of writing the condition are one condition");
    // A reference that decodes to what the writer refuses is refused as that character is.
    let risky = html.replace("&#58;", "&lt;");
    assert!(write(&risky).is_err(), "a `<` written as a reference in a condition went out");
}

#[test]
fn a_sheet_whose_media_was_past_the_attributes_kept_is_refused() {
    let many: String = (0..40).map(|i| format!(" data-a{i}=\"\"")).collect();
    let html = format!(
        "<html><head><style{many} media=\"(prefers-color-scheme: dark)\">.card {{ color: red }}</style></head><body><p class=\"card\">Priya Raman</p></body></html>"
    );
    assert_eq!(write(&html), Err(AuthoredError::Unsupported), "a dark-window rule would go out for every window");
    assert!(rules(&html).contains(&"MC-CSS-001"), "refused at save: {:?}", rules(&html));
    // Control: the same sheet with its condition among the attributes kept.
    let kept = format!(
        "<html><head><style media=\"(prefers-color-scheme: dark)\"{many}>.card {{ color: red }}</style></head><body><p class=\"card\">Priya Raman</p></body></html>"
    );
    let written = write(&kept).expect("written");
    assert!(written.head.contains(r#"media="(prefers-color-scheme: dark)""#), "{}", written.head);
}

#[test]
fn an_attribute_test_the_wrapper_cant_pass_is_refused_at_every_level() {
    for sheet in [r#"[class="card"] { color: red }"#, r#"*[class="card"] { color: red }"#, r#"[dir="rtl"] p { color: red }"#] {
        let html = format!("<html><head><style>{sheet}</style></head><body class=\"card\" dir=\"rtl\"><p>Priya Raman</p></body></html>");
        assert_eq!(write(&html), Err(AuthoredError::Unsupported), "{sheet}");
        assert!(rules(&html).contains(&"MC-CSS-001"), "{sheet}: refused at save: {:?}", rules(&html));
    }
    // Control: an attribute test of an element inside the signature is kept.
    let html = r#"<html><head><style>p[class="card"] { color: red }</style></head><body><p class="card">Priya Raman</p></body></html>"#;
    let written = write(html).expect("written");
    assert!(written.head.contains(r#".mc-test p[class="card"] { color: red }"#), "{}", written.head);
}

#[test]
fn a_selector_ending_in_a_space_that_isnt_css_white_space_names_another_class_in_the_reader_too() {
    let html = "<html><head><style>.card\u{a0} { color: #ff0000 }</style></head><body><p class=\"card\">Priya Raman</p></body></html>";
    assert_eq!(colour(html, "Priya Raman"), None, "the reader takes `.card\u{a0}` for `.card`, as no CSS reader does");
    let not = "<html><head><style>p:not(.card\u{a0}) { color: #ff0000 }</style></head><body><p class=\"card\">Priya Raman</p></body></html>";
    assert_eq!(colour(not, "Priya Raman"), Some(Rgb(0xff, 0, 0)), "the reader takes `:not(.card\u{a0})` for `:not(.card)`");
    // Control: CSS's own white space round a selector goes, as before.
    let spaced = "<html><head><style> \t.card\n { color: #ff0000 }</style></head><body><p class=\"card\">Priya Raman</p></body></html>";
    assert_eq!(colour(spaced, "Priya Raman"), Some(Rgb(0xff, 0, 0)));
    let spaced_not =
        "<html><head><style>p:not( .card\t) { color: #ff0000 }</style></head><body><p class=\"card\">Priya Raman</p><p>Ada</p></body></html>";
    assert_eq!(colour(spaced_not, "Priya Raman"), None);
    assert_eq!(colour(spaced_not, "Ada"), Some(Rgb(0xff, 0, 0)));
}

/// Whether MailCraft's reader shows the words `text` of `html` in the pane of a window `env`.
fn shown_in(html: &str, text: &str, env: &Env) -> bool {
    span(&parse_in(html, env).blocks, text).is_some()
}

/// Narrow and wide windows, light and dark.
fn windows() -> [Env; 4] {
    [Env::for_pane(320.0, false), Env::for_pane(320.0, true), Env::for_pane(1200.0, false), Env::for_pane(1200.0, true)]
}

/// The reviewer's finding (tokenizer.rs:323): a `media` made only of spaces that aren't CSS white
/// space (a no-break space, an em space, a vertical tab) was trimmed to nothing, which is no media,
/// and the sheet hid the words in every window where a CSS reader applies it in none.
#[test]
fn a_media_of_spaces_that_arent_css_white_space_holds_for_no_window() {
    let with = |media: &str| {
        format!(
            "<html><head><style media=\"{media}\">.y {{ display: none }}</style></head><body><p class=\"y\">Ada Lovelace</p><p>Priya Raman</p></body></html>"
        )
    };
    for media in ["&nbsp;", "&#160;", "&#11;", "&#x2003;", "\u{a0}", "\u{2003}&nbsp;"] {
        let html = with(media);
        for (n, env) in windows().iter().enumerate() {
            assert!(shown_in(&html, "Ada Lovelace", env), "{media:?}: the pane hid what no window's sheet hides (window {n})");
        }
        assert!(quotable(&html).contains("Ada Lovelace"), "{media:?}: the quote leaves out what the pane shows");
        assert!(to_text(&html).contains("Ada Lovelace"), "{media:?}: the plain text leaves out what the pane shows");
    }
    // Control: CSS's own white space alone is no media at all, and the sheet is for every window.
    for media in ["", " ", "&#12;", "&#9;&#10;", "\t"] {
        let html = with(media);
        for (n, env) in windows().iter().enumerate() {
            assert!(!shown_in(&html, "Ada Lovelace", env), "{media:?}: a sheet for every window hides it (window {n})");
            assert!(shown_in(&html, "Priya Raman", env), "{media:?}: fixture guard (window {n})");
        }
        assert!(!quotable(&html).contains("Ada Lovelace"), "{media:?}: quoted what the pane hides");
        assert!(!to_text(&html).contains("Ada Lovelace"), "{media:?}: the plain text has what the pane hides");
    }
}

#[test]
fn a_media_of_spaces_that_arent_css_white_space_is_written_with_its_sheet() {
    let with = |media: &str| {
        format!("<html><head><style media=\"{media}\">.card {{ color: red }}</style></head><body><p class=\"card\">Priya Raman</p></body></html>")
    };
    for (media, kept) in [("&nbsp;", "\u{a0}"), ("&#160;", "\u{a0}"), ("&#x2003;", "\u{2003}"), ("\u{a0}", "\u{a0}")] {
        let written = write(&with(media)).expect("written");
        assert!(
            written.head.contains(&format!("<style media=\"{kept}\">")),
            "{media:?}: the condition was dropped, and the sheet would go for every window: {}",
            written.head
        );
    }
    // Control: CSS's own white space alone is no condition, and none is written.
    let written = write(&with("&#12; ")).expect("written");
    assert!(written.head.contains("<style>") && !written.head.contains("media="), "{}", written.head);
}

/// The reviewer's nit (authored_sheet_agreement.rs:51-64): of a `media` written twice the first
/// counts, whatever comes past the attributes kept.
#[test]
fn of_a_media_written_twice_the_first_kept_counts() {
    let many: String = (0..40).map(|i| format!(" data-a{i}=\"\"")).collect();
    let doc =
        |attrs: &str| format!("<html><head><style{attrs}>.card {{ color: red }}</style></head><body><p class=\"card\">Priya Raman</p></body></html>");
    // The second, past the cap, is one HTML drops anyway.
    let written = write(&doc(&format!(" media=\"(min-width: 600px)\"{many} media=\"print\""))).expect("written");
    assert!(written.head.contains(r#"<style media="(min-width: 600px)">"#), "{}", written.head);
    assert!(!written.head.contains("print"), "{}", written.head);
    // One in capitals past the cap is as unknown as one in small letters.
    assert_eq!(write(&doc(&format!("{many} MEDIA=\"print\""))), Err(AuthoredError::Unsupported));
    // A kept empty `media` is the one that counts: the sheet is for every window, as written.
    let written = write(&doc(&format!(" media=\"\"{many} media=\"print\""))).expect("written");
    assert!(written.head.contains("<style>") && !written.head.contains("media="), "{}", written.head);
}
