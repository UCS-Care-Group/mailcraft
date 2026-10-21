// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Synthetic messages that imitate real mail, for tests here and in the crates that draw mail.
//!
//! Nothing here is copied from a real message: every person, company, product and address is
//! invented (`.example` domains), and the markup is written the way each kind of mail program
//! writes it. The files are in `crates/html/corpus/`. Pictures are referred to by `cid:` and are
//! not here: whoever needs their bytes draws them in code ([`PICTURES`] lists what to draw).

/// A reply written in Outlook (Word's HTML): `MsoNormal` paragraphs with empty paragraphs for
/// blank lines, list paragraphs with written-out markers, a bordered Word table, a signature with
/// a `cid:` logo, and two earlier messages quoted under "From:/Sent:" blocks with a line above.
pub const OUTLOOK_REPLY: &str = include_str!("../corpus/outlook_reply.html");

/// A reply written in Gmail: a `<div>` per line, a signature, and a thread quoted three
/// `<blockquote>`s deep.
pub const GMAIL_REPLY: &str = include_str!("../corpus/gmail_reply.html");

/// A shop's newsletter: nested layout tables, a hidden preheader, remote pictures (a wordmark, a
/// hero, products, icons), a button made of a table cell, tracked links, two tracking pixels.
pub const NEWSLETTER: &str = include_str!("../corpus/newsletter.html");

/// A newsletter the way a column builder writes it for every mail program at once: each section
/// a cell with a font size of nothing round blocks that set their own, the same thing written
/// twice where Outlook needs it otherwise (a button and a picture drawn for Outlook in comments
/// only it reads, the ones for everybody else hidden from it alone), a background picture given
/// three ways over a colour, pictures whose addresses leave the scheme out, write it in
/// capitals, have a space or a line break in them, or get their size from the style alone, a
/// part that is nothing but slices of one picture, `srcset`, and three tracking pixels.
pub const BUILT_NEWSLETTER: &str = include_str!("../corpus/built_newsletter.html");

/// A directory's weekly digest, the way a notification service writes one: a 600-pixel table
/// centred three ways (`<center>`, `align`, a centred cell), a grid of four tiles whose number
/// and icon stand side by side, written four ways (a table each way round, a picture in a line of
/// text, a paragraph), a two-column table whose long cells must wrap at the widths it gives, a
/// button, and a footer of small linked pictures side by side (a row of cells, then a line).
pub const ACCESS_DIGEST: &str = include_str!("../corpus/access_digest.html");

/// A payments summary whose widths are all in `style`: a wrapper table with a `width="100"`
/// attribute that its style makes 100%, a 600-pixel card centred by `margin: 0 auto`, and cells
/// with percentage widths (#150).
pub const STYLE_WIDTH_TABLE: &str = include_str!("../corpus/style_width_table.html");

/// A shop's mailing in a `<div>` card: `max-width:600px; margin:0 auto`, with a narrower block
/// inside it (#150).
pub const STYLE_CARD: &str = include_str!("../corpus/style_card.html");

/// An order confirmation: a grid of order facts, and a data table with a header row, right-aligned
/// amounts, spanning total rows and lines under the rows.
pub const RECEIPT: &str = include_str!("../corpus/receipt.html");

/// A message with screenshots pasted in (`cid:`), small pictures inside a line of text, a picture
/// that was never attached, one that is damaged, and a signature with a logo beside the text.
pub const SCREENSHOTS: &str = include_str!("../corpus/screenshots.html");

/// A calendar's invitation notice: a framed card, a grid of when/where/who, a numbered list with
/// a list inside it, and answer buttons.
pub const CALENDAR: &str = include_str!("../corpus/calendar.html");

/// A plain-text thread: long lines, quotes three `>` deep, addresses, and a signature after `-- `.
pub const PLAIN_THREAD: &str = include_str!("../corpus/plain_thread.txt");

/// Every way a message can try to make its reader fetch something or run something.
pub const HOSTILE_REMOTE: &str = include_str!("../corpus/hostile_remote.html");

/// Colours and sizes that try to hide text, or to make it unreadable.
pub const HOSTILE_COLOURS: &str = include_str!("../corpus/hostile_colours.html");

/// What a `cid:` picture of the corpus is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PictureKind {
    Png,
    Jpeg,
    Gif,
    /// Attached, but its bytes are not a picture.
    Damaged,
}

/// The pictures the corpus refers to: content id, kind, and size in pixels. One more id is
/// referred to and deliberately attached nowhere: [`MISSING_PICTURE`].
pub const PICTURES: [(&str, PictureKind, u32, u32); 7] = [
    ("image001.png@01DC3A1B.5F0C2E40", PictureKind::Png, 336, 96),
    ("f3b1c2d4-export-dialog@quillon.example", PictureKind::Png, 1040, 600),
    ("a77e0919-expected@quillon.example", PictureKind::Jpeg, 640, 380),
    ("tick-16@quillon.example", PictureKind::Gif, 32, 32),
    ("cross-16@quillon.example", PictureKind::Gif, 32, 32),
    ("quillon-mark@quillon.example", PictureKind::Png, 112, 112),
    ("damaged-capture@quillon.example", PictureKind::Damaged, 0, 0),
];

/// A content id [`SCREENSHOTS`] refers to that no part of the message has.
pub const MISSING_PICTURE: &str = "never-attached@quillon.example";

/// The HTML messages of the corpus that imitate ordinary mail, by name.
pub fn messages() -> Vec<(&'static str, &'static str)> {
    vec![
        ("outlook reply", OUTLOOK_REPLY),
        ("gmail reply", GMAIL_REPLY),
        ("newsletter", NEWSLETTER),
        ("receipt", RECEIPT),
        ("screenshots", SCREENSHOTS),
        ("calendar notice", CALENDAR),
        ("built newsletter", BUILT_NEWSLETTER),
        ("access digest", ACCESS_DIGEST),
        ("style width table", STYLE_WIDTH_TABLE),
        ("style card", STYLE_CARD),
    ]
}

/// HTML written to break whatever reads it, by name: the two hostile files, and generated cases
/// that are large in one direction each (depth, cells, sizes, spans, words). Each stays under
/// [`MAX_INPUT`](crate::MAX_INPUT) or is cut there.
pub fn hostile() -> Vec<(&'static str, String)> {
    let unclosed = "<b><i><u><font color=red size=7><span style='color:#0a0;background:#ff0'><a href='https://example.org/x'>open <table><tr><td bgcolor=#eef>cell \
                    <ul><li>item <p>para <h1>head <pre>pre <blockquote>quote <div style='background:#fee;border:1px solid red'>box <center>mid ";
    let absurd = "<table width=999999999 border=99999 cellpadding=99999><tr><td width='5000%' colspan=99999 rowspan=99999 height=99999>\
                  <font size=999 color='#12345678901234567890'>FONT-999</font> <span style='font-size:99999px'>PX-99999</span> \
                  <span style='font-size:0.0001em'>EM-TINY</span> <span style='font-size:1e9pt'>PT-1E9</span> <span style='font-size:-50px'>NEGATIVE</span>\
                  <img src='cid:huge' width=999999 height=999999 alt='HUGE-PICTURE'><img src='cid:wide' width=100000 height=3 alt='WIDE-PICTURE'>\
                  <ol start=4294967295><li>LAST-NUMBER<li>ONE-MORE</ol><ol start=-7 type=i><li value=99999999999>ROMAN</ol>\
                  <hr width=99999 size=99999><p style='margin:99999px;text-indent:-99999px;line-height:99999'>MARGINS</p></td></tr></table>";
    let wide_row: String = (0..200).map(|i| format!("<td>Column{i}WithAVeryLongUnbreakableHeading{}</td>", "x".repeat(40))).collect();
    let strange = format!(
        "<p>BIDI \u{202e}desrever si siht\u{202c} and \u{2066}isolated\u{2069} text</p><p>ZALGO z{}</p><p>ZERO-WIDTH a{}b</p>\
         <p>ASTRAL \u{1f980}\u{1f469}\u{200d}\u{1f680} \u{10348} \u{e0001}</p><p>NUL a\u{0}b &#0; &#xD800; &#x110000; &#-1;</p><p dir=rtl>\u{5e9}\u{5dc}\u{5d5}\u{5dd} RTL \u{645}\u{631}\u{62d}\u{628}\u{627}</p>",
        "\u{301}\u{316}\u{334}".repeat(600),
        "\u{200b}\u{200d}\u{feff}".repeat(2000),
    );
    vec![
        ("remote", HOSTILE_REMOTE.to_string()),
        ("colours", HOSTILE_COLOURS.to_string()),
        ("deep divs", format!("{}DEEP-DIVS{}", "<div>".repeat(30_000), "</div>".repeat(30_000))),
        ("deep tables", format!("{}DEEP-TABLES", "<table bgcolor='#eeeeff' border=1><tr><td>level ".repeat(6_000))),
        ("deep quotes", (0..3_000).map(|i| format!("<blockquote>quote level {i} ")).collect()),
        ("deep lists", (0..3_000).map(|i| format!("<ul><li>list level {i} ")).collect()),
        (
            "deep boxes",
            (0..3_000u64).map(|i| format!("<div style='background:#{:06x};border-left:3px solid'>box {i} ", (i * 7919) % 0xff_ffff)).collect(),
        ),
        ("unclosed", unclosed.repeat(1_500)),
        ("a million cells", format!("<table border=1>{}</table>", "<tr><td>a</td><td>b</td><td>c</td><td>d</td></tr>".repeat(40_000))),
        ("one row", format!("<table border=1><tr>{}", "<td>x".repeat(300_000))),
        ("wide row", format!("<table border=1><tr>{wide_row}</tr><tr>{wide_row}</tr></table>")),
        ("absurd sizes", absurd.to_string()),
        (
            "long words",
            format!(
                "<p>{}</p><p><a href='https://example.org/{}'>https://example.org/{}</a></p>",
                "W".repeat(150_000),
                "u".repeat(60_000),
                "u".repeat(60_000)
            ),
        ),
        ("many spans", (0..40_000u64).map(|i| format!("<span style='color:#{:06x}'>s{i} </span>", (i * 104_729) % 0xff_ffff)).collect()),
        ("one paragraph", format!("<p>{}</p>", "word ".repeat(300_000))),
        ("many breaks", format!("line<br>{}end", "<br>line".repeat(150_000))),
        (
            "many pictures",
            (0..8_000).map(|i| format!("<img src='cid:p{i}' alt='p{i}'><img src='https://example.org/{i}.png' width=40 height=40>")).collect(),
        ),
        ("many rules", "<hr>text".repeat(100_000)),
        ("strange text", strange),
        ("preformatted", format!("<pre>{}</pre>", "    indented\tcode line with    spaces\n".repeat(20_000))),
    ]
}

/// A newsletter of about `bytes` bytes, built the way mailing services build them: nested layout
/// tables, long inline styles, tracked links and remote pictures around rather little text.
pub fn large_newsletter(bytes: usize) -> String {
    let mut out = String::with_capacity(bytes.saturating_add(8_192));
    out.push_str(
        "<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>The Long Read</title><style>body{margin:0}.x{display:none}@media(max-width:600px){.c{width:100%!important}}</style></head>\
         <body style=\"margin:0;padding:0;background-color:#f3f1ec;\"><div style=\"display:none;max-height:0;overflow:hidden;\">This week: everything, at length.</div>\
         <table role=\"presentation\" width=\"100%\" cellpadding=\"0\" cellspacing=\"0\" border=\"0\" bgcolor=\"#f3f1ec\"><tr><td align=\"center\" style=\"padding:20px 10px;\">\
         <table role=\"presentation\" class=\"c\" width=\"640\" cellpadding=\"0\" cellspacing=\"0\" border=\"0\" bgcolor=\"#ffffff\" style=\"width:640px;background-color:#ffffff;\">\
         <tr><td align=\"center\" bgcolor=\"#1d3557\" style=\"padding:24px;background-color:#1d3557;font-family:Georgia,serif;font-size:28px;color:#ffffff;\">The Long Read</td></tr>",
    );
    let mut n = 0usize;
    while out.len() < bytes {
        n += 1;
        let pad = "0".repeat(120);
        out.push_str(&format!(
            "<tr><td style=\"padding:26px 32px 6px 32px;font-family:Georgia,'Times New Roman',serif;font-size:22px;line-height:28px;color:#1d3557;mso-line-height-rule:exactly;\">\
             <a href=\"https://click.longread.example/t/{n}/h?u=4410&amp;s={pad}\" style=\"color:#1d3557;text-decoration:none;\">Story {n}: the harbour at low water</a></td></tr>\
             <tr><td style=\"padding:0 32px;\"><a href=\"https://click.longread.example/t/{n}/i?u=4410&amp;s={pad}\"><img src=\"https://img.longread.example/2026/10/story-{n}-wide.jpg?w=1152&amp;q=70&amp;sig={pad}\" width=\"576\" height=\"240\" alt=\"Picture for story {n}\" style=\"display:block;width:100%;max-width:576px;height:auto;border:0;outline:none;text-decoration:none;\"></a></td></tr>\
             <tr><td style=\"padding:14px 32px 0 32px;font-family:Helvetica,Arial,sans-serif;font-size:15px;line-height:23px;color:#33363b;mso-line-height-rule:exactly;\">\
             <p style=\"margin:0 0 12px 0;\">At low water the harbour gives back what it took in winter: a ladder, two anchors, the frame of a bicycle. <strong style=\"color:#1d3557;\">Story {n}</strong> follows the people who go down with rakes before the tide turns.</p>\
             <p style=\"margin:0 0 12px 0;\">They keep a ledger. <span style=\"color:#8d99ae;font-size:13px;\">Nine minutes to read.</span></p></td></tr>\
             <tr><td style=\"padding:4px 32px 18px 32px;\"><table role=\"presentation\" cellpadding=\"0\" cellspacing=\"0\" border=\"0\"><tr>\
             <td bgcolor=\"#e63946\" style=\"background-color:#e63946;border-radius:3px;\"><a href=\"https://click.longread.example/t/{n}/b?u=4410&amp;s={pad}\" style=\"display:inline-block;padding:10px 22px;font-family:Helvetica,Arial,sans-serif;font-size:14px;font-weight:bold;color:#ffffff;text-decoration:none;\">Read story {n}</a></td>\
             <td width=\"12\" style=\"font-size:1px;line-height:1px;\">&nbsp;</td>\
             <td style=\"font-family:Helvetica,Arial,sans-serif;font-size:13px;color:#8d99ae;\"><a href=\"https://click.longread.example/t/{n}/s?u=4410&amp;s={pad}\" style=\"color:#8d99ae;text-decoration:underline;\">Save for later</a></td></tr></table></td></tr>\
             <tr><td style=\"padding:0 32px;\"><table role=\"presentation\" width=\"100%\" cellpadding=\"0\" cellspacing=\"0\" border=\"0\"><tr><td height=\"1\" style=\"border-top:1px solid #e5e2da;font-size:1px;line-height:1px;\">&nbsp;</td></tr></table></td></tr>\
             <!--[if mso]><tr><td height=\"8\" style=\"font-size:8px;line-height:8px;\">&nbsp;</td></tr><![endif]-->"
        ));
    }
    out.push_str(
        "<tr><td align=\"center\" style=\"padding:22px 32px;font-family:Helvetica,Arial,sans-serif;font-size:11px;line-height:17px;color:#8d99ae;\">You asked for The Long Read at reader@inbox.example. \
         <a href=\"https://click.longread.example/u?u=4410\" style=\"color:#8d99ae;\">Unsubscribe</a></td></tr></table></td></tr></table>\
         <img src=\"https://click.longread.example/o/4410.gif\" width=\"1\" height=\"1\" alt=\"\"></body></html>",
    );
    out
}

#[cfg(test)]
mod tests;
