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
    let text = "Yes.

On Mon, Ada Quinn wrote:
> Will you come?
> Ada
";
    assert_eq!(kept(&text_blocks(text)).trim(), "Yes.");
}

#[test]
fn a_quote_at_the_end_that_nothing_introduces_is_a_citation_and_kept() {
    let html = "<p>As the manual says:</p><blockquote><p>Turn the valve slowly.</p></blockquote>";
    assert_eq!(start(&parse(html)), None);
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
    use crate::history::MAX_BLOCKS;
    // Its own text, `fill` paragraphs, then an Outlook header: `blocks` blocks in all.
    let message = |fill: usize| {
        let mut html = String::from("<p>Mine.</p>");
        for _ in 0..fill {
            html.push_str("<p>x</p>");
        }
        html.push_str("<p>From: Ada</p><p>Sent: Monday</p><p>To: Ben</p>");
        parse(&html)
    };
    let fill = MAX_BLOCKS - message(0).len();
    // At the limit, the header is found where it is.
    let at = message(fill);
    assert_eq!(at.len(), MAX_BLOCKS);
    assert_eq!(start(&at), Some(MAX_BLOCKS - 3));
    // One block more, and the message is shown whole.
    let over = message(fill + 1);
    assert_eq!(over.len(), MAX_BLOCKS + 1);
    assert_eq!(start(&over), None);
}

#[test]
fn an_answer_after_many_blank_lines_is_the_senders_own() {
    let blank = "
"
    .repeat(40);
    let html = format!("<p>See below.</p><p>On Mon, Ada Quinn wrote:</p><blockquote><p>What happened?</p></blockquote><pre>{blank}Answer</pre>");
    assert_eq!(start(&parse(&html)), None);
    // In plain text too.
    let text = format!(
        "See below.

On Mon, Ada Quinn wrote:
> What happened?
{blank}Answer
"
    );
    assert_eq!(start(&text_blocks(&text)), None);
}

#[test]
fn a_forward_keeps_what_it_forwards() {
    use crate::history::start_in;
    // A note over an Outlook header, in a message whose subject is a forward's.
    let html = "<p>FYI</p><p>From: Ada Quinn</p><p>Sent: Monday</p><p>To: Ben</p><p>Subject: Plan</p><p>The plan.</p>";
    assert_eq!(start_in(&parse(html), "FW: Plan"), None);
    assert_eq!(start_in(&parse(html), "Fwd: Plan"), None);
    assert_eq!(start_in(&parse(html), "WG: Plan"), None);
    // The same as a reply is cut; a reply to a forward too.
    assert_eq!(start_in(&parse(html), "RE: Plan"), Some(1));
    assert_eq!(start_in(&parse(html), "RE: FW: Plan"), Some(1));
    // Gmail's and Apple Mail's forwards say so in a line, whatever the subject.
    let html = "<p>FYI</p><p>---------- Forwarded message ---------</p><p>From: Ada</p><p>Date: Mon</p><p>To: Ben</p><p>The plan.</p>";
    assert_eq!(start(&parse(html)), None);
    let text = "See this.

Begin forwarded message:

From: Ada
Date: Mon
To: Ben

The plan.
";
    assert_eq!(start(&text_blocks(text)), None);
}

#[test]
fn a_separator_and_the_header_in_one_block_are_found() {
    // Plain-text Outlook: the line of underscores, then From: on the next line, no blank line.
    let text = "Thanks.

________________________________
From: Ada Quinn
Sent: Monday
To: Ben
Subject: Plan

Old text
";
    assert_eq!(kept(&text_blocks(text)).trim(), "Thanks.");
}

#[test]
fn a_hostile_message_is_read_in_linear_time() {
    // About 8000 "On a wrote:" lines, a quote, then one block of 130,000 whitespace spans ending
    // in a letter: each candidate once rescanned that block, a billion spans in all. Read once,
    // it takes milliseconds; the bound is loose for slow, emulated and instrumented runs.
    let mut html = String::from("<p>Mine.</p>");
    for _ in 0..8_000 {
        html.push_str("<p>On a wrote:</p>");
    }
    html.push_str("<blockquote><p>q</p></blockquote><pre>");
    for i in 0..130_000 {
        html.push_str(if i % 2 == 0 { "<b> </b>" } else { "<i> </i>" });
    }
    html.push_str("x</pre>");
    let blocks = parse(&html);
    let began = std::time::Instant::now();
    let _ = start(&blocks);
    let took = began.elapsed();
    assert!(took < std::time::Duration::from_secs(3), "{took:?}");
}

#[test]
fn an_answer_in_the_quote_right_under_a_header_is_kept() {
    let html = "<p>Hi Ada,</p><hr><div><b>From:</b> Ada<br><b>Sent:</b> Monday<br><b>To:</b> Ben</div>        <blockquote><p>Can you check the numbers?</p></blockquote><p>They look fine.</p>";
    assert_eq!(start(&parse(html)), None);
    // In paragraphs of their own, with a Cc line.
    let html =
        "<p>Hi,</p><p>From: Ada</p><p>Sent: Monday</p><p>To: Ben</p><p>Cc: Kirk</p>        <blockquote><p>Numbers?</p></blockquote><p>Fine.</p>";
    assert_eq!(start(&parse(html)), None);
}

#[test]
fn a_marker_in_a_quote_the_sender_answered_in_doesnt_cut_it() {
    // Answered in the quote under the header: an Original Message line further down the quote,
    // with an answer under it, is part of what the sender answered.
    let html = "<p>Hi,</p><hr><div><b>From:</b> Ada<br><b>Sent:</b> Monday<br><b>To:</b> Ben</div>        <blockquote><p>Numbers?</p><p>-----Original Message-----</p><p>Old.</p></blockquote><p>Fine.</p>";
    assert_eq!(start(&parse(html)), None);
    // And under an On … wrote: line.
    let html = "<p>Hi,</p><p>On Monday, Ada wrote:</p><blockquote><p>Numbers?</p></blockquote><p>Fine.</p>        <blockquote><p>-----Original Message-----</p></blockquote><p>Also fine.</p>";
    assert_eq!(start(&parse(html)), None);
}

#[test]
fn quotes_in_the_older_messages_under_a_header_dont_keep_them() {
    // The message answered was itself a Gmail reply: its own quote and signature are history.
    let html = "<p>Done.</p><hr><div><b>From:</b> Ada<br><b>Sent:</b> Monday<br><b>To:</b> Ben</div>        <p>Sure.</p><p>On Sunday, Ben wrote:</p><blockquote><p>Can you?</p></blockquote><p>Ada</p>";
    assert_eq!(kept(&parse(html)), "Done.");
}
