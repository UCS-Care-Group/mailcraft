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

/// A club bulletin in a rounded, framed card: padding and margins in px, a dashed notice and a
/// dotted one with rounded corners, a dashed divider, a line height of 1.7, and a table of padded cells
/// with a line under each row (#71).
pub const STYLE_BOXES: &str = include_str!("../corpus/style_boxes.html");

/// A charity's open-day notice that lays things out as boxes on a line (`display: inline-block`,
/// #597): a small picture at the start of a long sentence, two 260-pixel columns that stand side
/// by side inside a `font-size:0` row, and three links drawn as buttons of different looks (filled,
/// framed, tinted) that wrap when the line is full.
pub const STYLE_INLINE: &str = include_str!("../corpus/style_inline.html");

/// A coffee roaster's letter whose whole look is in a `<style>` block (#598): reset rules, a
/// hidden preheader, descendant, child, adjacent, attribute and structural selectors,
/// `!important`, `:visited` and `:hover` rules that must not apply, `@import`, `@font-face`
/// and `@media` that must do nothing, and mobile and desktop copies of one line.
pub const STYLE_SHEET_NEWSLETTER: &str = include_str!("../corpus/style_sheet_newsletter.html");

/// A dairy's newsletter with a desktop copy and a mobile copy of one paragraph, switched by
/// `@media (max-width: 480px)`, a column width that changes, a note for panes of 40em and more, one
/// for panes of 600 pixels and less written as `@media` inside `@media`, and blocks for `print` and
/// `tv` that never apply (#599).
pub const RESPONSIVE_NEWSLETTER: &str = include_str!("../corpus/responsive_newsletter.html");

/// A newsletter whose style sheet has a palette for dark windows (`@media (prefers-color-scheme:
/// dark)`) beside the light one, and a dark edition and a light edition of one paragraph (#599).
pub const DARK_NEWSLETTER: &str = include_str!("../corpus/dark_newsletter.html");

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
        ("style boxes", STYLE_BOXES),
        ("style inline", STYLE_INLINE),
        ("style sheet newsletter", STYLE_SHEET_NEWSLETTER),
        ("responsive newsletter", RESPONSIVE_NEWSLETTER),
        ("dark newsletter", DARK_NEWSLETTER),
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
        ("many inline boxes", "<span style='display:inline-block;padding:2px 9px;background:#eef'>b</span> ".repeat(8_000)),
        (
            "deep inline boxes",
            format!("{}DEEP-INLINE{}", "<div style='display:inline-block;padding:1px;border:1px solid red'>".repeat(5_000), "</div>".repeat(5_000)),
        ),
        ("strange text", strange),
        (
            "sheet deep selectors",
            format!(
                "<style>{}{{color:red}}{}{{color:red}}</style>{}x",
                vec!["div"; 16].join(" "),
                vec!["div.a"; 17].join(" > "),
                "<div class=a>".repeat(20_000)
            ),
        ),
        (
            "sheet many rules",
            format!(
                "<style>{}</style>{}",
                (0..9_000).map(|i| format!("p.c{i} span, .x{i}:not(.y) + b{{color:#{:06x}}}", i * 4099)).collect::<String>(),
                "<p class=c1><span>s</span></p>".repeat(30_000)
            ),
        ),
        (
            "sheet siblings",
            format!(
                "<style>.a ~ .b ~ .c ~ .d ~ .e, .a + .a + .a + .z, :not(.q):nth-child(2n+1){{color:red}}</style><p>SIBLINGS</p>{}",
                "<i class=a></i>".repeat(100_000)
            ),
        ),
        (
            "sheet not list",
            format!("<style>*:not({}){{color:#f00}}</style><p>VISIBLE</p>{}", vec![".a"; 20_000].join(","), "<b>x</b>".repeat(150_000)),
        ),
        (
            "sheet not list at the cap",
            format!("<style>*:not({}){{color:#f00}}</style><p>VISIBLE</p>{}", vec![".a"; 32].join(","), "<b>x</b>".repeat(150_000)),
        ),
        (
            "sheet many simples",
            format!(
                "<style>*:not({}){{color:#f00}}</style><p>VISIBLE</p>{}",
                (0..9_000).map(|i| format!(".z{i}")).collect::<Vec<_>>().join(","),
                "<i class='a b c d e f g h i j k l m n o p'>x</i>".repeat(40_000)
            ),
        ),
        (
            "sheet attribute fold",
            format!(
                "<style>{}</style><p>VISIBLE</p>{}",
                (0..2_000).map(|i| format!("[title*=q{i} i]{{color:#f00}}")).collect::<String>(),
                format!("<i title='{}'>x</i>", "AbCdEfGh".repeat(12)).repeat(20_000)
            ),
        ),
        ("sheet stray ends", format!("<style>p{{color:#f00}}</style><p>VISIBLE</p>{}{}", "<i>".repeat(600), "</b>".repeat(450_000))),
        (
            "sheet two long rules",
            format!(
                "<style>*{{{}}} i{{{}}}</style><p>VISIBLE</p>{}",
                (0..40).map(|i| format!("margin-left:{i}px;")).collect::<String>(),
                (0..40).map(|i| format!("padding-top:{i}px !important;")).collect::<String>(),
                "<i>x</i>".repeat(285_000)
            ),
        ),
        (
            "sheet not walls",
            format!(
                "<style>{}</style><p>VISIBLE</p>{}",
                (0..20)
                    .map(|k| format!(
                        "*:not({}){{color:#f00}}",
                        (0..32).map(|i| format!(".a.b.c.d.e.f.g.h.i.j.k.l.m.n.o.p.z{k}x{i}")).collect::<Vec<_>>().join(",")
                    ))
                    .collect::<String>(),
                "<i class='a b c d e f g h i j k l m n o p'>x</i>".repeat(20_000)
            ),
        ),
        (
            "sheet many looks",
            format!(
                "<style>{}{{{}}}</style><p>VISIBLE</p>{}",
                (0..4_000).map(|i| format!(".c{i}")).collect::<Vec<_>>().join(","),
                ["margin-top", "margin-left", "margin-bottom", "margin-right", "padding-top", "padding-left", "padding-right", "padding-bottom"]
                    .iter()
                    .flat_map(|p| (0..5).map(move |i| format!("{p}:{i}.5px;")))
                    .collect::<String>()
                    + &format!("font-family:{};", "abcdefghij".repeat(5)),
                (0..40_000).map(|i| format!("<i class=c{}>x</i>", i % 4_000)).collect::<String>()
            ),
        ),
        ("sheet look per element", look_per_element()),
        ("sheet many properties", many_properties()),
        ("sheet junk", format!("<style>{}</style><p>x</p>", "{([\\\"'/*@:,>+~!;".repeat(8_000))),
        ("preformatted", format!("<pre>{}</pre>", "    indented\tcode line with    spaces\n".repeat(20_000))),
    ]
}

/// Newsletters of the kind a mailing builder writes, with the style sheet such builders write:
/// resets, `#templateBodyN .mcnTextContent p`, `a[x-apple-data-detectors]`, `u + #body a`,
/// `[owa] .x`, `*[class="mcnTextContent"]`, utility classes, `@media`, nested tables, and a
/// `.mobile-only{display:none}` line near the end. About 65, 105 and 145 KB, with 160 to 300
/// rules and 1,000 to 2,300 elements. Every word is invented (`.example` and `example.org`).
pub fn realistic_newsletters() -> Vec<(&'static str, String)> {
    vec![
        ("builder newsletter, small", builder_newsletter(65, 20)),
        ("builder newsletter, medium", builder_newsletter(100, 30)),
        ("builder newsletter, large", builder_newsletter(140, 45)),
    ]
}

/// What [`realistic_newsletters`] hides with its sheet, and what it shows after it.
pub const BUILDER_HIDDEN: &str = "MOBILE ONLY COPY";
pub const BUILDER_FOOTER: &str = "FOOTER STAYS VISIBLE";

fn builder_newsletter(sections: usize, groups: usize) -> String {
    let mut sheet = String::from(
        "body,table,td,p,a,li,blockquote{-webkit-text-size-adjust:100%;-ms-text-size-adjust:100%}\n\
         table,td{mso-table-lspace:0pt;mso-table-rspace:0pt}\nimg{-ms-interpolation-mode:bicubic;border:0}\n\
         body{margin:0;padding:0;background-color:#f2efe9;color:#3a3330}\n\
         a[x-apple-data-detectors]{color:inherit !important;text-decoration:none !important}\n\
         u + #body a{color:inherit;text-decoration:none}\n[owa] .x{display:block}\n\
         *[class=\"mcnTextContent\"]{font-family:Georgia,serif}\n.mobile-only{display:none}\n\
         table.wrap{width:600px;margin:0 auto;background-color:#fff}\n",
    );
    for i in 0..40 {
        sheet.push_str(&format!(".p-{i}{{padding:{}px}} .t-{i}{{font-size:{}px;line-height:1.4}}\n", i % 24, 12 + i % 8));
    }
    for i in 0..groups {
        sheet.push_str(&format!(
            "#templateBody{i} .mcnTextContent,#templateBody{i} .mcnTextContent p{{color:#{:06x};text-align:left}}\n\
             #templateBody{i} .mcnTextContent a{{color:#8a3b12;font-weight:bold}}\n\
             #templateBody{i} td.btn{{background-color:#8a3b12;padding:10px 20px;text-align:center}}\n\
             table.section{i} > tbody > tr > td{{padding:8px 16px}}\n",
            (i * 7919) % 0x00ff_ffff
        ));
    }
    sheet.push_str("@media only screen and (max-width:480px){table.wrap{width:100% !important}.mobile-only{display:block !important}.desktop-only{display:none !important}}\n");
    let mut html = format!(
        "<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>The Harbour Lane letter</title><style type=\"text/css\">{sheet}</style></head>\
         <body id=\"body\"><table class=\"wrap\" role=\"presentation\" cellpadding=\"0\" cellspacing=\"0\" width=\"100%\"><tr><td>"
    );
    for i in 0..sections {
        let g = i % groups;
        html.push_str(&format!(
            "<table class=\"section{g} p-{p}\" role=\"presentation\" width=\"100%\" cellpadding=\"0\" cellspacing=\"0\"><tbody><tr>\
             <td id=\"templateBody{g}\" class=\"p-{p}\"><table role=\"presentation\" width=\"100%\"><tbody><tr><td class=\"mcnTextContent t-{t}\">\
             <p>Harbour Lane Bakery, loaf {i}: a long-rise sourdough with rye, scored by hand and baked at dawn on the quay. \
             <a href=\"https://bakery.example.org/loaf/{i}\">Read about loaf {i}</a>, or <a href=\"https://bakery.example.org/order/{i}\">order it</a>.</p>\
             <p class=\"t-{t}\">Collect from the counter before nine, or have it left in the box by the gate. <span class=\"x\">Fresh daily.</span></p></td></tr></tbody></table>\
             <table role=\"presentation\" align=\"center\"><tbody><tr><td class=\"btn\"><a href=\"https://bakery.example.org/buy/{i}\">Buy loaf {i}</a></td></tr></tbody></table>\
             </td></tr></tbody></table>",
            p = i % 40,
            t = (i * 3) % 40
        ));
    }
    html.push_str(&format!(
        "<p class=\"mobile-only\">{BUILDER_HIDDEN}</p><p class=\"desktop-only\">Read on a computer.</p><p>{BUILDER_FOOTER}</p></td></tr></table></body></html>"
    ));
    html
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

/// 4 rules, each a list of 1,000 short classes, with 40 `margin-left` declarations each, and about
/// 9,000 elements of 16 of the 4,000 classes each: every element gets a look of its own to build.
fn look_per_element() -> String {
    let decls: String = (0..40).map(|i| format!("margin-left:{i}px;")).collect();
    let mut html = String::from("<style>");
    for r in 0..4 {
        let list = (0..1_000).map(|i| format!(".k{}", r * 1_000 + i)).collect::<Vec<_>>().join(",");
        html.push_str(&format!("{list}{{{decls}}}"));
    }
    html.push_str("</style><p>VISIBLE</p>");
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    for _ in 0..9_000 {
        html.push_str("<i class='");
        for _ in 0..16 {
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            html.push_str(&format!("k{} ", (seed >> 33) % 4_000));
        }
        html.push_str("'>x</i>");
    }
    html
}

/// 20 rules, each a list of 150 short classes with one declaration of a property of its own, and
/// elements of 16 of the 3,000 classes each: every look says about 16 declarations.
fn many_properties() -> String {
    let props = [
        "margin-top:3px",
        "margin-left:4px",
        "margin-right:5px",
        "margin-bottom:6px",
        "padding-top:3px",
        "padding-left:4px",
        "padding-right:5px",
        "padding-bottom:6px",
        "color:#336699",
        "background-color:#eeeeee",
        "border-top:1px solid #333",
        "border-left:1px solid #333",
        "border-right:1px solid #333",
        "border-bottom:1px solid #333",
        "font-size:13px",
        "line-height:1.4",
        "text-align:center",
        "font-weight:bold",
        "text-decoration:underline",
        "width:50%",
    ];
    let mut html = String::from("<style>");
    for (r, prop) in props.iter().enumerate() {
        let list = (0..150).map(|i| format!(".m{}", r * 150 + i)).collect::<Vec<_>>().join(",");
        html.push_str(&format!("{list}{{{prop}}}"));
    }
    html.push_str("</style><p>VISIBLE</p>");
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    for _ in 0..10_500 {
        html.push_str("<i class='");
        for _ in 0..16 {
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            html.push_str(&format!("m{} ", (seed >> 33) % 3_000));
        }
        html.push_str("'>x</i>");
    }
    html
}
