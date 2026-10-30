// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;
use crate::corpus;

/// Every attribute in `html`, read back with the tokenizer: name and value.
fn attributes(html: &str) -> Vec<(String, String)> {
    Tokenizer::new(html)
        .filter_map(|t| match t {
            Token::Open(tag) => Some(tag.attributes().map(|(n, v)| (n.to_ascii_lowercase(), v)).collect::<Vec<_>>()),
            _ => None,
        })
        .flatten()
        .collect()
}

fn attribute_names(html: &str) -> Vec<String> {
    attributes(html).into_iter().map(|(n, _)| n).collect()
}

/// Every link and picture address in `html` is one that may be followed or shown.
fn addresses_are_safe(html: &str) -> bool {
    attributes(html).iter().all(|(n, v)| !matches!(n.as_str(), "href" | "src") || is_safe_url(v))
}

/// What never goes out, whatever came in.
fn assert_inert(out: &str, what: &str) {
    let lower = out.to_ascii_lowercase();
    for bad in ["<script", "<style", "<iframe", "<object", "<embed", "<svg", "<form", "<input", "<link", "<meta", "<base", "javascript:", "<!--"] {
        assert!(!lower.contains(bad), "{what}: {bad} in {}", out.get(..2000).unwrap_or(out));
    }
    assert!(addresses_are_safe(out), "{what}");
    for name in attribute_names(out) {
        assert!(!name.starts_with("on") && !["srcset", "background", "ping", "formaction", "hidden"].contains(&name.as_str()), "{what}: {name}");
    }
}

#[test]
fn layout_and_pictures_keep_their_look() {
    let html = "<table width=\"600\" cellpadding=\"0\" style=\"border-collapse:collapse\" bgcolor=\"#ffffff\"><tr>\
                <td align=\"center\" valign=\"top\" style=\"color:#336;font-family:Calibri\" colspan=\"2\"><img src=\"cid:logo@news.example\" alt=\"Logo\" width=\"120\" height=\"40\"></td></tr>\
                <tr><td><p class=\"MsoNormal\"><b>Five</b> <font color=\"#c00\" face=\"Arial\" size=\"2\">new</font> arrivals<br></p></td>\
                <td><ul><li>one</li><li>two</li></ul><a href=\"https://shop.example/a?b=1&amp;c=2\" title=\"Shop\">Shop</a></td></tr></table>";
    assert_eq!(quotable(html), html);
    // Unquoted values are quoted, names are lower case, and a remote picture of a size stays.
    assert_eq!(
        quotable("<P ALIGN=right>x</P><IMG SRC=https://example.org/a.png WIDTH=40>"),
        "<p align=\"right\">x</p><img src=\"https://example.org/a.png\" width=\"40\">"
    );
}

#[test]
fn only_the_body_goes() {
    let html = "<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>T</title><style>.a{color:red}</style>\
                <link rel=stylesheet href=https://evil.example/x.css><base href=https://evil.example/></head>\
                <body bgcolor=#fff onload=\"x()\"><p>Hello</p></body></html>";
    assert_eq!(quotable(html), "<p>Hello</p>");
    // Without a body, the whole document; a head with no end goes all the same.
    assert_eq!(quotable("<p>a</p>"), "<p>a</p>");
    assert_eq!(quotable("<head><title>t</title><p>gone"), "");
    assert_eq!(quotable("before<body>after<body>again"), "afteragain");
}

#[test]
fn what_runs_or_embeds_goes_with_its_content() {
    for (html, kept) in [
        ("a<script>alert(1)</script>b", "ab"),
        ("a<SCRIPT src=https://evil.example/x.js></SCRIPT>b", "ab"),
        ("a<style>p{color:red}</style>b", "ab"),
        ("a<iframe src=https://evil.example>fallback</iframe>b", "ab"),
        ("a<object data=x.swf><param name=a value=b>fallback</object>b", "ab"),
        ("a<embed src=x.swf>b", "ab"),
        ("a<applet code=x>fallback</applet>b", "ab"),
        ("a<frameset><frame src=x></frameset>b", "ab"),
        ("a<svg><image href=https://evil.example/s.png /></svg>b", "ab"),
        ("a<math><mi>x</mi></math>b", "ab"),
        ("a<template><img src=https://evil.example/t.png></template>b", "ab"),
        ("a<noscript><img src=https://evil.example/n.png></noscript>b", "ab"),
        ("a<button>Press</button><select><option>1<option>2</select><textarea>t</textarea><input value=v>b", "ab"),
        ("a<link rel=preload href=x><meta http-equiv=refresh content=0><base href=x>b", "ab"),
        ("a<!-- <img src=https://evil.example/c.png> -->b<!--[if mso]><p>word</p><![endif]-->", "ab"),
    ] {
        assert_eq!(quotable(html), kept, "{html}");
    }
    // A form's own tag goes and its controls with it; what it lays out stays.
    assert_eq!(quotable("<form action=https://evil.example/c><p>Text</p><input name=x></form>"), "<p>Text</p>");
    // Unknown elements lose their tags and keep their text.
    assert_eq!(quotable("<o:p>x</o:p><marquee>m</marquee><video src=v.mp4>fallback</video>"), "xmfallback");
}

#[test]
fn what_the_sender_hid_goes_with_its_content() {
    for (html, kept) in [
        ("a<div style=\"display:none\">preheader <b>x</b></div>b", "ab"),
        ("a<span style=\"VISIBILITY: hidden\">x</span>b", "ab"),
        ("a<div style=\"max-height:0;overflow:hidden\">x</div>b", "ab"),
        ("a<div hidden><div>inner</div>still hidden</div>b", "ab"),
        ("a<img src=\"cid:x\" hidden>b", "ab"),
        ("a<o:p style=\"display:none\">x</o:p>b", "ab"),
    ] {
        assert_eq!(quotable(html), kept, "{html}");
    }
    // An element whose end tag is left out ends where the next one starts.
    assert_eq!(quotable("<p hidden>x<p>y</p>"), "<p>y</p>");
    assert_eq!(quotable("<table><tr><td hidden>x<td>y</td></tr></table>"), "<table><tr><td>y</td></tr></table>");
    assert_eq!(quotable("<ul><li style=display:none>x<li>y</ul>"), "<ul><li>y</li></ul>");
    assert_eq!(
        quotable("<table><tr><td>a</td><td hidden>x</tr><tr><td>b</td></tr></table>"),
        "<table><tr><td>a</td></tr><tr><td>b</td></tr></table>"
    );
}

#[test]
fn attributes_go_by_list() {
    assert_eq!(quotable("<p onclick=\"x()\" OnMouseOver=y id=a data-x=1 xmlns=z>t</p>"), "<p id=\"a\">t</p>");
    assert_eq!(quotable("<td background=\"https://evil.example/bg.gif\" bgcolor=#eee>t</td>"), "<td bgcolor=\"#eee\">t</td>");
    assert_eq!(quotable("<img src=cid:a srcset=\"https://evil.example/2x.png 2x\" loading=lazy>"), "<img src=\"cid:a\">");
    assert_eq!(quotable("<a href=mailto:a@example.org ping=https://evil.example/p target=_blank>m</a>"), "<a href=\"mailto:a@example.org\">m</a>");
    // The first of a name counts.
    assert_eq!(quotable("<p align=left align=right>t</p>"), "<p align=\"left\">t</p>");
    // Values are escaped again.
    assert_eq!(quotable("<p title='say \"hi\" &amp; <go>'>t</p>"), "<p title=\"say &quot;hi&quot; &amp; &lt;go&gt;\">t</p>");
}

#[test]
fn links_lead_only_where_links_may() {
    for bad in
        ["javascript:alert(1)", "  JAVA&#9;SCRIPT&#58;alert(1)", "vbscript:x", "data:text/html,x", "file:///etc/passwd", "/relative", "ms-msdt:x"]
    {
        assert_eq!(quotable(&format!("<a href=\"{bad}\">t</a>")), "<a>t</a>", "{bad}");
    }
    for good in ["https://example.org/", "http://example.org", "mailto:a@example.org", "tel:+15550100"] {
        assert_eq!(quotable(&format!("<a href=\"{good}\">t</a>")), format!("<a href=\"{good}\">t</a>"));
    }
    // `href` belongs to links only.
    assert_eq!(quotable("<div href=https://example.org>t</div>"), "<div>t</div>");
}

#[test]
fn pictures_from_the_message_or_the_web_and_never_trackers() {
    for gone in [
        "<img src=\"https://evil.example/p.gif\" width=\"1\" height=\"1\">",
        "<img src=\"https://evil.example/p.gif\" width=2>",
        "<img src=\"https://evil.example/p.gif\" style=\"width:1px;height:1px\">",
        "<img src=\"https://evil.example/p.gif\" height=\"0\">",
        "<img src=\"data:image/gif;base64,R0lGOD\">",
        "<img src=\"file:///c:/secret.png\">",
        "<img src=\"\\\\evil.example\\share\\s.png\">",
        "<img src=\"javascript:x\">",
        "<img alt=no-address>",
        "<img src=\"https://evil.example/x.png\" style=\"display:none\">",
    ] {
        assert_eq!(quotable(gone), "", "{gone}");
    }
    assert_eq!(quotable("<img src=\"cid:dot\" width=1>"), "<img src=\"cid:dot\" width=\"1\">");
    assert_eq!(quotable("<img src=\"HTTPS://example.org/hero.jpg\" width=600>"), "<img src=\"HTTPS://example.org/hero.jpg\" width=\"600\">");
    // An address is read the way the reading pane reads it, and carried that way.
    assert_eq!(quotable("<img src=\"//example.org/a.png\">"), "<img src=\"https://example.org/a.png\">");
    assert_eq!(quotable("<img src=\" https://example.org/\r\nb.png\">"), "<img src=\"https://example.org/b.png\">");
}

#[test]
fn styles_that_fetch_or_run_go_whole() {
    for bad in [
        "background:url(https://evil.example/bg.gif)",
        "background: URL ( 'x' )",
        "background:u\\72l(x)",
        "background:\\75 rl(x)",
        "background:u/**/rl(x)",
        "width:expression(alert(1))",
        "@import 'x'",
        "behavior:url(x.htc)",
        "-moz-binding:url(x)",
        "background-image:image-set('x' 1x)",
        "x:java\tscript:alert(1)",
    ] {
        assert_eq!(quotable(&format!("<p style=\"{bad}\">t</p>")), "<p>t</p>", "{bad}");
    }
    assert_eq!(quotable("<p style=\"color:#336;font-family:'Segoe UI'\">t</p>"), "<p style=\"color:#336;font-family:&#39;Segoe UI&#39;\">t</p>");
}

#[test]
fn text_is_escaped_again() {
    assert_eq!(quotable("a &lt;script&gt; &amp; b < c &nbsp;\u{e9}"), "a &lt;script&gt; &amp; b &lt; c \u{a0}\u{e9}");
    // `xmp` is raw text: its markup is text, shown as such.
    assert_eq!(quotable("<xmp><b>x</b></xmp>"), "&lt;b&gt;x&lt;/b&gt;");
    assert_eq!(quotable("\r\nline\r\n"), "\r\nline\r\n");
}

#[test]
fn tags_left_open_are_closed_and_stray_ends_dropped() {
    assert_eq!(quotable("<b><i>x"), "<b><i>x</i></b>");
    assert_eq!(quotable("x</b></div></table>"), "x");
    assert_eq!(quotable("<b><i>x</b>y</i>"), "<b><i>x</i></b>y");
    assert_eq!(quotable("<p>a<p>b<div>c</div>"), "<p>a</p><p>b</p><div>c</div>");
    assert_eq!(quotable("<ul><li>a<li>b</ul>"), "<ul><li>a</li><li>b</li></ul>");
    assert_eq!(quotable("<table><tr><td>a<td>b<tr><td>c</table>x"), "<table><tr><td>a</td><td>b</td></tr><tr><td>c</td></tr></table>x");
    // An end tag inside a cell doesn't reach out of the cell.
    assert_eq!(quotable("<div><table><tr><td>a</div>b</td></tr></table></div>"), "<div><table><tr><td>ab</td></tr></table></div>");
    // Void elements are never closed.
    assert_eq!(quotable("a<br>b<hr><wbr></br>"), "a<br>b<hr><wbr>");
}

#[test]
fn nesting_is_capped() {
    let deep = format!("{}x{}", "<div>".repeat(10_000), "</div>".repeat(10_000));
    let out = quotable(&deep);
    assert_eq!(out.matches("<div>").count(), MAX_DEPTH);
    assert_eq!(out.matches("</div>").count(), MAX_DEPTH);
    assert!(out.contains('x'));
}

#[test]
fn quoting_again_changes_nothing() {
    for (name, html) in corpus::messages() {
        let once = quotable(html);
        assert_eq!(quotable(&once), once, "{name}");
    }
}

#[test]
fn the_corpus_keeps_its_text_and_loses_its_trackers() {
    for (name, html) in corpus::messages() {
        let out = quotable(html);
        assert_inert(&out, name);
        // What a reader sees of it is the same.
        assert_eq!(crate::to_text(&out), crate::to_text(html), "{name}");
    }
    let remote = quotable(corpus::HOSTILE_REMOTE);
    assert!(!remote.contains("evil.example/pixel.gif") && !remote.contains("HIDDEN-ONE") && !remote.contains("phone home.</p><script"));
    for kept in ["VISIBLE-ONE", "VISIBLE-THREE", "VISIBLE-FOUR", "VISIBLE-FIVE", "https://evil.example/plain.gif", "https://evil.example/click"] {
        assert!(remote.contains(kept), "{kept}: {remote}");
    }
    for gone in [
        "url(",
        "body-bg",
        "attr-bg",
        "table-bg",
        "onload",
        "payload",
        "frame.html",
        "set-1x",
        "input.gif",
        "collect",
        "noscript.gif",
        "template.gif",
    ] {
        assert!(!remote.contains(gone), "{gone}: {remote}");
    }
}

#[test]
fn the_hostile_corpus_stays_inert_and_bounded() {
    for (name, html) in corpus::hostile() {
        let out = quotable(&html);
        assert_inert(&out, name);
        assert!(out.len() <= html.len().min(MAX_INPUT).saturating_mul(6).saturating_add(MAX_DEPTH * 16), "{name}");
    }
}

#[test]
fn hostile_runs_are_linear() {
    for s in ["<".repeat(300_000), "<a ".repeat(100_000), "<div hidden>".repeat(100_000), "<p>".repeat(200_000), "<td>".repeat(200_000)] {
        let out = quotable(&s);
        assert!(out.len() <= s.len().saturating_mul(6));
    }
}

proptest::proptest! {
    #[test]
    fn any_input_quotes_without_anything_that_runs(s in "[<>/!a-z =\"'&;:()\\\\-]{0,120}") {
        let out = quotable(&s);
        let lower = out.to_ascii_lowercase();
        proptest::prop_assert!(!lower.contains("<script") && !lower.contains("<style") && !lower.contains("<iframe"), "{}", out);
        proptest::prop_assert!(attribute_names(&out).iter().all(|n| !n.starts_with("on")), "{}", out);
    }

    #[test]
    fn tag_soup_quotes_without_anything_that_runs(parts in proptest::collection::vec(proptest::sample::select(vec![
        "<script>", "</script>", "<style>", "<iframe>", "<svg>", "<p>", "</p>", "<div hidden>", "</div>", "<td>", "<tr>", "<table>", "</table>",
        "<a href='javascript:x'>", "<img src=x onerror=y>", "<!--", "-->", "<body>", "x", "<", ">", "\"", "'", "<b onclick=", "<textarea>",
        "</textarea>", "<xmp>", "</xmp>", "<SCRIPT ", "<scr", "ipt>", "<style ", "&lt;script&gt;",
    ]), 0..40)) {
        let s: String = parts.concat();
        let out = quotable(&s);
        let lower = out.to_ascii_lowercase();
        proptest::prop_assert!(!lower.contains("<script") && !lower.contains("<style") && !lower.contains("<iframe"), "{} -> {}", s, out);
        proptest::prop_assert!(addresses_are_safe(&out), "{} -> {}", s, out);
        proptest::prop_assert!(attribute_names(&out).iter().all(|n| !n.starts_with("on")), "{}", out);
    }
}

/// #598: what the sender hid with a style sheet goes with its content, and the sheet is not carried.
#[test]
fn what_a_style_sheet_hid_is_not_carried() {
    let html = "<style>.pre{display:none}.m{visibility:hidden}</style><div class=pre>PREHEADER</div><p class=m>MOBILE</p><p class=x>KEPT</p>";
    let out = quotable(html);
    assert!(out.contains("KEPT") && !out.contains("PREHEADER") && !out.contains("MOBILE") && !out.contains("<style"), "{out}");
    assert_eq!(quotable(&out), out);
}
