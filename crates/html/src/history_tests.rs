// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Where the quoted history of a reply begins (#1778), in replies as Outlook, Gmail and plain-text
//! programs write them, and where it must not be cut.

use crate::history::start;
use crate::{Block, parse, text_blocks};

/// The text of the blocks before the history, joined.
fn kept(blocks: &[Block]) -> String {
    let upto = start(blocks).unwrap_or(blocks.len());
    blocks.iter().take(upto).flat_map(|b| b.spans.iter().map(|s| s.text.as_str())).collect::<Vec<_>>().join(" ")
}

#[test]
fn an_outlook_reply_keeps_its_own_text_and_drops_the_header_and_what_follows() {
    let html = r#"<div>Hi Ada,</div><div>Good catch, fixed.</div><div>Thanks,<br>Ben</div>
        <div id="appendonsend"></div><hr style="display:inline-block;width:98%">
        <div id="divRplyFwdMsg"><b>From:</b> Ada Quinn &lt;ada@example.org&gt;<br><b>Sent:</b> Thursday, 19 November 2026 22:54<br>
        <b>To:</b> Ben Ray &lt;ben@example.org&gt;<br><b>Subject:</b> RE: Idea</div>
        <div>Kirk said PM01 has been down all day.</div>"#;
    let blocks = parse(html);
    let cut = start(&blocks).expect("history found");
    let text = kept(&blocks);
    assert!(text.contains("Good catch") && text.contains("Ben"), "{text}");
    assert!(!text.contains("From:") && !text.contains("Kirk"), "{text}");
    // The rule over the header goes with it.
    assert!(blocks.get(cut).is_some_and(|b| matches!(b.kind, crate::BlockKind::Rule) || b.spans.iter().any(|s| s.text.contains("From"))));
}

#[test]
fn a_header_in_paragraphs_of_their_own_and_in_other_languages_is_found() {
    let html =
        "<p>Ok.</p><p>________________________________</p><p>Von: Ada Quinn</p><p>Gesendet: Montag</p><p>An: Ben</p><p>Betreff: Plan</p><p>Alt.</p>";
    assert_eq!(kept(&parse(html)), "Ok.");
    let html = "<p>Merci.</p><p>De : Ada</p><p>Envoyé : lundi</p><p>À : Ben</p><p>Objet : Plan</p><p>Ancien.</p>";
    assert_eq!(kept(&parse(html)), "Merci.");
}

#[test]
fn gmail_and_apple_mail_introduce_the_quote_with_a_wrote_line() {
    let html = r#"<div dir="ltr">Sounds good.</div><br><div class="gmail_quote"><div dir="ltr" class="gmail_attr">On Thu, Nov 19, 2026 at 4:20 PM Ada Quinn &lt;ada@example.org&gt; wrote:<br></div>
        <blockquote class="gmail_quote">Can you confirm?</blockquote></div>"#;
    assert_eq!(kept(&parse(html)).trim(), "Sounds good.");
}

#[test]
fn original_message_and_plain_text_quotes_are_found() {
    let text = "Done.\n\n-----Original Message-----\nFrom: Ada\nSent: Monday\nTo: Ben\nSubject: Plan\n\nOld text\n";
    assert_eq!(kept(&text_blocks(text)).trim(), "Done.");
    let html = "<p>Yes.</p><blockquote><p>Will you come?</p><p>Ada</p></blockquote>";
    assert_eq!(kept(&parse(html)), "Yes.");
}

#[test]
fn answers_written_between_quotes_are_kept_whole() {
    let html = "<p>See below.</p><blockquote><p>Question one?</p></blockquote><p>Answer one.</p><blockquote><p>Question two?</p></blockquote><p>Answer two.</p>";
    assert_eq!(start(&parse(html)), None);
}

#[test]
fn a_message_that_is_all_history_or_has_none_is_shown_whole() {
    // Nothing of its own before the header: a forward with nothing written.
    let html = "<p>From: Ada</p><p>Sent: Monday</p><p>To: Ben</p><p>Hello.</p>";
    assert_eq!(start(&parse(html)), None);
    let html = "<p>Lunch on Friday?</p><p>From the team.</p>";
    assert_eq!(start(&parse(html)), None);
    // A line beginning "From:" without the rest of a header is the sender's own text.
    let html = "<p>Notes.</p><p>From: the plant, three readings.</p><p>More.</p>";
    assert_eq!(start(&parse(html)), None);
    assert_eq!(start(&[]), None);
}

#[test]
fn hostile_sizes_are_bounded() {
    // Very many blocks and a very long line: looked at within the caps, no panic.
    let mut html = String::from("<p>Hi.</p>");
    for _ in 0..10_000 {
        html.push_str("<p>x</p>");
    }
    html.push_str(&format!("<p>From: {}</p><p>Sent: now</p><p>To: you</p>", "a".repeat(100_000)));
    let _ = start(&parse(&html));
    let _ = start(&text_blocks(&"On ".repeat(50_000)));
}

#[test]
fn answers_written_in_the_quote_under_a_wrote_line_are_kept() {
    let html = "<p>See below.</p><p>On Mon, Ada Quinn wrote:</p><blockquote><p>Question one?</p></blockquote><p>Answer one.</p>";
    assert_eq!(start(&parse(html)), None);
    // A top-posted reply with nothing written in the quote is still cut at the line.
    let html = "<p>Yes.</p><p>On Mon, Ada Quinn wrote:</p><blockquote><p>Will you come?</p></blockquote>";
    assert_eq!(kept(&parse(html)), "Yes.");
}

#[test]
fn a_picture_or_a_table_after_the_quote_is_the_senders_own() {
    let html = r#"<p>Here it is.</p><blockquote><p>Can you send the chart?</p></blockquote><p><img src="cid:chart" width="200" height="100"></p>"#;
    assert_eq!(start(&parse(html)), None);
    let html = "<p>Numbers:</p><blockquote><p>How many?</p></blockquote><table><tr><td>12</td><td>14</td></tr></table>";
    assert_eq!(start(&parse(html)), None);
}

#[test]
fn a_message_longer_than_what_is_looked_at_is_shown_whole() {
    // Its own text, a long quote, and an answer past the blocks looked at.
    let mut html = String::from("<p>Mine.</p>");
    for _ in 0..crate::history::MAX_BLOCKS {
        html.push_str("<blockquote><p>q</p></blockquote>");
    }
    html.push_str("<p>Answer at the end.</p>");
    assert_eq!(start(&parse(&html)), None);
}
