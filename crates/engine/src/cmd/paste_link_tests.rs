// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Pasted web and mail addresses become links in an HTML message and in a signature (#1793).
//!
//! Plain text or HTML that only shows an address (`https://`, `http://`, `www.`, or
//! `name@host`) is a link the owner can follow. The words they see are that address, so the
//! plain-text part does not say it again in angle brackets. Typing on afterwards is not part of
//! the link, and one Undo takes the whole paste away. An address that is already a link keeps
//! the target it had. A plain-text message, an unsafe scheme, and a string that is not an
//! address stay ordinary text.

use mailcraft_model::RichText;
use mailcraft_time::{Date, DateTime};
use serde_json::{Value, json};

use crate::{Compose, Session, sample};

/// Longest address a paste makes into a link. A longer one stays text, as it does when plain
/// text is shown (`mailcraft_html`'s cap for the same addresses).
const MAX_PASTED_URL: usize = 2048;

fn session() -> Session {
    let now = DateTime::new(Date::new(2026, 10, 14).unwrap(), 16, 0, 0).unwrap().to_timestamp(0);
    Session::at(sample::mailbox(now, 0), now, 0)
}

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

fn compose(s: &Session) -> &Compose {
    s.compose.as_ref().unwrap()
}

/// What was written above the signature the sample account adds.
fn mine(s: &Session) -> String {
    let text = compose(s).body.plain();
    text.split("\n\nAvery Quinn").next().unwrap_or("").to_string()
}

fn new_message() -> Session {
    let mut s = session();
    run(&mut s, "mail.new", json!({"to": "maya@lumen.example", "subject": "Links"}));
    run(&mut s, "compose.select", json!({"from": 0}));
    s
}

fn paste_text(s: &mut Session, text: &str) {
    run(s, "edit.paste", json!({"text": text}));
}

/// `https://example.org/` followed by enough `a`s to make `n` bytes.
fn url_of_len(n: usize) -> String {
    let prefix = "https://example.org/";
    format!("{prefix}{}", "a".repeat(n.saturating_sub(prefix.len())))
}

/// Where `shown` sits in `text`, as a character range.
fn place(text: &RichText, shown: &str) -> (usize, usize) {
    let plain = text.text();
    let byte = plain.find(shown).unwrap_or_else(|| panic!("{shown:?} is not in {plain:?}"));
    let start = plain[..byte].chars().count();
    (start, start + shown.chars().count())
}

/// `shown` is one link, to `href`, and nothing past its last character belongs to it.
fn require_link(text: &RichText, shown: &str, href: &str) {
    let (start, end) = place(text, shown);
    let mid = start + shown.chars().count() / 2;
    assert_eq!(text.link_span(mid), Some((start, end)), "{shown} in {}", text.text());
    let linked = text.all(start, end, |style| style.link.as_deref() == Some(href)).unwrap_or_else(|e| panic!("{e}"));
    assert!(linked, "{shown:?} should lead to {href}");
}

/// None of `shown` is a link.
fn require_unlinked(text: &RichText, shown: &str) {
    let (start, end) = place(text, shown);
    let plain = text.all(start, end, |style| style.link.is_none()).unwrap_or_else(|e| panic!("{e}"));
    assert!(plain, "{shown:?} should stay ordinary text in {}", text.text());
}

fn assert_no_links(text: &RichText) {
    let linked: Vec<_> = text.paragraphs().iter().flat_map(|p| &p.runs).filter_map(|run| run.style.link.clone()).collect();
    assert!(linked.is_empty(), "unexpected links {linked:?} in {}", text.text());
}

/// The anchor a sent HTML part uses for `shown` leading to `href`.
fn anchor(href: &str, shown: &str) -> String {
    let escape = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;");
    format!("<a href=\"{}\">{}</a>", escape(href), escape(shown))
}

#[test]
fn pasted_bare_urls_are_links_when_the_message_is_sent() {
    // A bare address is linked as itself. `www.` is written out as https. The scheme's letters
    // stay as they were pasted.
    let pasted = "\
https://example.org/path?a=1&b=2#frag
http://example.org
www.example.org
HTTP://EXAMPLE.ORG";
    let cases = [
        ("https://example.org/path?a=1&b=2#frag", "https://example.org/path?a=1&b=2#frag"),
        ("http://example.org", "http://example.org"),
        ("www.example.org", "https://www.example.org"),
        ("HTTP://EXAMPLE.ORG", "HTTP://EXAMPLE.ORG"),
    ];
    let mut s = new_message();
    paste_text(&mut s, pasted);
    assert_eq!(mine(&s), pasted);
    let text = compose(&s).body.text();
    for (shown, href) in cases {
        require_link(text, shown, href);
    }
    // The words already are the address, so plain text does not add `<href>` beside them.
    let plain = text.to_plain();
    for (shown, href) in cases {
        assert_eq!(plain.matches(shown).count(), 1, "{plain}");
        assert!(!plain.contains(&format!("<{href}>")), "{plain}");
    }

    let id = run(&mut s, "compose.send", json!({}))["id"].as_u64().unwrap();
    let m = s.mailbox.message(id).unwrap();
    let sent = m.body.text.as_str();
    let html = m.body.html.as_deref().unwrap_or("");
    for (shown, href) in cases {
        assert_eq!(sent.matches(shown).count(), 1, "{sent}");
        assert!(!sent.contains(&format!("<{href}>")), "{sent}");
        assert!(html.contains(&anchor(href, shown)), "missing {} in {html}", anchor(href, shown));
    }
}

#[test]
fn prose_with_urls_punctuation_parentheses_and_mail_addresses_is_linked() {
    // The sentence's punctuation stays outside the link. A `)` closes the address only when the
    // address itself opened it. Each line is its own, and a mail address is a mailto link.
    let pasted = "\
See (https://example.org/a_(b))!
Also http://example.org, and www.example.org.
Write to ada@example.org.";
    let mut s = new_message();
    paste_text(&mut s, pasted);
    assert_eq!(mine(&s), pasted);
    let text = compose(&s).body.text();
    require_link(text, "https://example.org/a_(b)", "https://example.org/a_(b)");
    require_link(text, "http://example.org", "http://example.org");
    require_link(text, "www.example.org", "https://www.example.org");
    require_link(text, "ada@example.org", "mailto:ada@example.org");
    // The marks around those addresses belong to the sentence.
    assert!(pasted.contains("a_(b))!"));
    assert!(pasted.contains("example.org,"));
    assert!(pasted.contains("www.example.org."));
    assert!(pasted.contains("ada@example.org."));
    let after = |shown: &str| {
        let (_, end) = place(text, shown);
        text.all(end, end + 1, |style| style.link.is_none()).unwrap_or(false)
    };
    assert!(after("https://example.org/a_(b)"), "the closing ) was swallowed");
    assert!(after("http://example.org"), "the comma was swallowed");
    assert!(after("www.example.org"), "the full stop was swallowed");
    assert!(after("ada@example.org"), "the full stop was swallowed");
}

#[test]
fn pasting_a_url_replaces_the_selection_with_the_link() {
    let mut s = new_message();
    run(&mut s, "compose.type", json!({"text": "say Hello now"}));
    run(&mut s, "compose.select", json!({"from": 4, "to": 9}));
    paste_text(&mut s, "https://example.org/replaced");
    assert_eq!(mine(&s), "say https://example.org/replaced now");
    let text = compose(&s).body.text();
    require_link(text, "https://example.org/replaced", "https://example.org/replaced");
    require_unlinked(text, "say ");
    require_unlinked(text, " now");
}

#[test]
fn typing_after_a_pasted_url_does_not_grow_the_link() {
    let url = "https://example.org/pasted";
    let mut s = new_message();
    paste_text(&mut s, url);
    run(&mut s, "compose.type", json!({"text": " next"}));
    assert_eq!(mine(&s), format!("{url} next"));
    let text = compose(&s).body.text();
    require_link(text, url, url);
    // The caret was at the end of the address. What is typed there is not part of the link.
    require_unlinked(text, " next");
    let (_, end) = place(text, url);
    assert_eq!(text.link_span(end / 2), Some((0, end)));
    assert!(text.style_at(end + 1).link.is_none(), "the typed word took the link: {:?}", text.style_at(end + 1));
}

#[test]
fn one_undo_removes_the_pasted_link() {
    let mut s = new_message();
    let before = compose(&s).body.text().clone();
    paste_text(&mut s, "https://example.org/undo");
    require_link(compose(&s).body.text(), "https://example.org/undo", "https://example.org/undo");
    run(&mut s, "compose.undo", json!({}));
    assert_eq!(compose(&s).body.text(), &before);
    assert!(!compose(&s).body.can_undo(), "the paste was more than one undo step");
}

#[test]
fn pasted_html_links_a_bare_url_inside_a_style_and_keeps_an_existing_href() {
    let mut s = new_message();
    run(
        &mut s,
        "edit.paste",
        json!({"html": "<p><b>https://example.org/docs</b></p><p><b>Read https://example.org/guide today</b></p><p><a href=\"https://other.example\">https://example.org/keep</a></p>"}),
    );
    let text = compose(&s).body.text().clone();
    assert!(text.text().contains("https://example.org/docs"), "{}", text.text());
    assert!(text.text().contains("Read https://example.org/guide today"), "{}", text.text());
    assert!(text.text().contains("https://example.org/keep"), "{}", text.text());
    // The bold the paste brought is still bold, on the address and on the words around it.
    for shown in ["https://example.org/docs", "Read https://example.org/guide today"] {
        let (start, _) = place(&text, shown);
        let (from, to) = text.paragraph_span(start).unwrap_or_else(|| panic!("{shown} has no paragraph"));
        assert!(text.all(from, to, |style| style.bold).unwrap_or(false), "{shown} lost its bold");
    }
    require_unlinked(&text, "Read ");
    require_unlinked(&text, " today");
    // A link that already has a target keeps it, even when the words are some other address.
    let (start, end) = place(&text, "https://example.org/keep");
    let mid = start + "https://example.org/keep".chars().count() / 2;
    assert_eq!(text.link_span(mid), Some((start, end)));
    assert_eq!(text.style_at(mid + 1).link.as_deref(), Some("https://other.example"));
    require_link(&text, "https://example.org/docs", "https://example.org/docs");
    require_link(&text, "https://example.org/guide", "https://example.org/guide");
    let (guide_at, _) = place(&text, "https://example.org/guide");
    assert!(text.all(guide_at, guide_at + "https://example.org/guide".chars().count(), |style| style.bold).unwrap_or(false));
}

#[test]
fn pasted_html_links_an_address_whose_formatting_changes_part_way() {
    // Formatting that starts or ends inside an address does not cut the link short. Each part
    // keeps its own formatting. An existing link still ends the address.
    let mut s = new_message();
    run(
        &mut s,
        "edit.paste",
        json!({"html": "<p>See https://example.org/<b>notes</b> today</p><p><i>www.</i>example.org/guide, or ada@<u>example.org</u>.</p><p><a href=\"https://other.example\">https://example.org/</a>extra</p>"}),
    );
    let text = compose(&s).body.text().clone();
    require_link(&text, "https://example.org/notes", "https://example.org/notes");
    require_link(&text, "www.example.org/guide", "https://www.example.org/guide");
    require_link(&text, "ada@example.org", "mailto:ada@example.org");
    require_unlinked(&text, "See ");
    require_unlinked(&text, " today");
    require_unlinked(&text, ", or ");
    require_unlinked(&text, "extra");
    let (keep, _) = place(&text, "https://example.org/extra");
    assert_eq!(text.style_at(keep + 1).link.as_deref(), Some("https://other.example"));
    let styled = |shown: &str, has: fn(&mailcraft_model::rich::Style) -> bool| {
        let (start, end) = place(&text, shown);
        text.all(start, end, has).unwrap_or(false)
    };
    assert!(styled("notes", |style| style.bold), "notes lost its bold");
    assert!(styled("https://example.org/", |style| !style.bold), "the address before the bold became bold");
    assert!(styled("www.", |style| style.italic), "www. lost its italic");
    assert!(styled("example.org/guide", |style| !style.italic));
    assert!(styled("ada@", |style| !style.underline), "ada@ became underlined");
    let (at, _) = place(&text, "ada@example.org");
    assert!(text.all(at + 4, at + 15, |style| style.underline).unwrap_or(false), "example.org lost its underline");
}

#[test]
fn a_url_of_the_maximum_length_is_linked() {
    let url = url_of_len(MAX_PASTED_URL);
    assert_eq!(url.len(), MAX_PASTED_URL);
    let mut s = new_message();
    paste_text(&mut s, &url);
    let text = compose(&s).body.text();
    assert_eq!(text.text().lines().next().map(str::len), Some(MAX_PASTED_URL));
    assert_eq!(text.link_span(1), Some((0, MAX_PASTED_URL)));
    assert_eq!(text.style_at(1).link.as_deref().map(str::len), Some(MAX_PASTED_URL));
    assert_eq!(text.style_at(1).link.as_deref(), Some(url.as_str()));
}

#[test]
fn unsafe_schemes_and_non_addresses_stay_plain_text() {
    let long = url_of_len(MAX_PASTED_URL + 1);
    assert_eq!(long.len(), MAX_PASTED_URL + 1);
    let pasted = format!("javascript:alert(1)\nfile:///etc/passwd\nhttp://\n{long}");
    let mut s = new_message();
    paste_text(&mut s, &pasted);
    assert_eq!(mine(&s), pasted);
    let text = compose(&s).body.text();
    assert_no_links(text);
    assert!(text.text().lines().any(|line| line == "javascript:alert(1)"));
    assert!(text.text().lines().any(|line| line == "file:///etc/passwd"));
    assert!(text.text().lines().any(|line| line == "http://"));
    assert!(text.text().lines().any(|line| line == long));
}

#[test]
fn a_plain_text_message_leaves_a_pasted_url_unlinked() {
    let mut s = new_message();
    run(&mut s, "compose.formatPlain", json!({}));
    run(&mut s, "compose.select", json!({"from": 0}));
    paste_text(&mut s, "https://example.org/plain");
    run(&mut s, "edit.paste", json!({"html": "<b>https://example.org/from-html</b>"}));
    let text = compose(&s).body.text();
    assert!(mine(&s).contains("https://example.org/plain"), "{}", mine(&s));
    assert!(mine(&s).contains("https://example.org/from-html"), "{}", mine(&s));
    require_unlinked(text, "https://example.org/plain");
    require_unlinked(text, "https://example.org/from-html");
    // The HTML arrived as words: the bold is not kept in a plain-text message, and neither is a link.
    let (start, _) = place(text, "https://example.org/from-html");
    assert!(text.all(start, start + "https://example.org/from-html".chars().count(), |style| !style.bold && style.link.is_none()).unwrap_or(false));
}

#[test]
fn a_url_pasted_into_an_open_signature_becomes_a_link() {
    // File › Signatures edits the signature with the same paste as a message (`signature.open`).
    let mut s = session();
    run(&mut s, "signature.open", json!({"name": "Standard"}));
    let before = s.signature_edit.as_ref().unwrap().body.text().clone();
    let end = before.len();
    run(&mut s, "compose.select", json!({"from": end}));
    paste_text(&mut s, "\nhttps://example.org/team");
    let text = s.signature_edit.as_ref().unwrap().body.text().clone();
    assert!(text.text().ends_with("https://example.org/team"), "{}", text.text());
    require_unlinked(&text, "Avery Quinn");
    require_link(&text, "https://example.org/team", "https://example.org/team");
    run(&mut s, "compose.undo", json!({}));
    assert_eq!(s.signature_edit.as_ref().unwrap().body.text(), &before);
    assert!(!s.signature_edit.as_ref().unwrap().body.can_undo());
}
