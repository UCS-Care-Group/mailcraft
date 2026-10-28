// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;
use crate::{Rgb, parse, to_text};

/// The colour of the first span of the first block.
fn colour(html: &str) -> Option<Rgb> {
    parse(html).first().and_then(|b| b.spans.first()).and_then(|s| s.color)
}

fn texts(html: &str) -> Vec<String> {
    parse(html).iter().map(|b| b.text()).collect()
}

/// Per block: whether its first span is red.
fn reds(html: &str) -> Vec<bool> {
    parse(html).iter().map(|b| b.spans.first().and_then(|s| s.color) == RED).collect()
}

const RED: Option<Rgb> = Some(Rgb(255, 0, 0));
const BLUE: Option<Rgb> = Some(Rgb(0, 0, 255));

#[test]
fn a_class_id_and_tag_style_text() {
    assert_eq!(colour("<style>.a{color:#f00}</style><p class=a>x</p>"), RED);
    assert_eq!(colour("<style>#a{color:#f00}</style><p id=a>x</p>"), RED);
    assert_eq!(colour("<style>p{color:#f00}</style><p>x</p>"), RED);
    assert_eq!(colour("<style>*{color:#f00}</style><p>x</p>"), RED);
    assert_eq!(colour("<style>.a{color:#f00}</style><p class=b>x</p>"), None);
    // Letter case in names, and several classes.
    assert_eq!(colour("<STYLE>P.Big{COLOR:#F00}</STYLE><p class='x BIG y'>x</p>"), RED);
}

#[test]
fn the_style_may_be_in_the_body_and_still_styles_what_came_before() {
    assert_eq!(colour("<p class=a>x</p><style>.a{color:#f00}</style>"), RED);
    assert_eq!(colour("<body><p class=a>x</p><style>.a{color:#f00}</style></body>"), RED);
}

#[test]
fn specificity_beats_order_and_order_breaks_ties() {
    assert_eq!(colour("<style>#a{color:#f00}.a{color:#00f}</style><p id=a class=a>x</p>"), RED);
    assert_eq!(colour("<style>.a{color:#f00}p{color:#00f}</style><p class=a>x</p>"), RED);
    assert_eq!(colour("<style>.a{color:#00f}.a{color:#f00}</style><p class=a>x</p>"), RED);
    assert_eq!(colour("<style>p.a{color:#f00}.a{color:#00f}</style><p class=a>x</p>"), RED);
    assert_eq!(colour("<style>.a.b{color:#f00}.a{color:#00f}</style><p class='a b'>x</p>"), RED);
}

#[test]
fn important_beats_everything_and_inline_beats_the_rest() {
    assert_eq!(colour("<style>p{color:#f00}</style><p style='color:#00f'>x</p>"), BLUE);
    assert_eq!(colour("<style>p{color:#f00 !important}</style><p style='color:#00f'>x</p>"), RED);
    assert_eq!(colour("<style>#a{color:#00f}p{color:#f00!important}</style><p id=a>x</p>"), RED);
    assert_eq!(colour("<style>p{color:#f00 ! IMPORTANT}</style><p style='color:#00f'>x</p>"), RED);
}

#[test]
fn combinators() {
    let m = |sheet: &str, body: &str| colour(&format!("<style>{sheet}</style>{body}"));
    assert_eq!(m("div p{color:#f00}", "<div><section><p>x</p></section></div>"), RED);
    assert_eq!(m("div>p{color:#f00}", "<div><section><p>x</p></section></div>"), None);
    assert_eq!(m("div > p{color:#f00}", "<div><p>x</p></div>"), RED);
    assert_eq!(reds("<style>h1+p{color:#f00}</style><h1>t</h1><p>x</p><p>y</p>"), [false, true, false]);
    assert_eq!(reds("<style>h1~p{color:#f00}</style><p>a</p><h1>t</h1><p>x</p><div>q</div><p>y</p>"), [false, false, true, false, true]);
}

#[test]
fn attribute_selectors() {
    let m = |sel: &str, attrs: &str| colour(&format!("<style>{sel}{{color:#f00}}</style><p {attrs}>x</p>"));
    assert_eq!(m("[data-x]", "data-x"), RED);
    assert_eq!(m("[data-x=a]", "data-x=a"), RED);
    assert_eq!(m("[data-x=\"a b\"]", "data-x='a b'"), RED);
    assert_eq!(m("[data-x=a]", "data-x=b"), None);
    assert_eq!(m("[data-x~=b]", "data-x='a b c'"), RED);
    assert_eq!(m("[lang|=en]", "lang=en-GB"), RED);
    assert_eq!(m("[data-x^=ab]", "data-x=abc"), RED);
    assert_eq!(m("[data-x$=bc]", "data-x=abc"), RED);
    assert_eq!(m("[data-x*=b]", "data-x=abc"), RED);
    assert_eq!(m("[data-x=abc i]", "data-x=ABC"), RED);
    assert_eq!(m("[data-x=abc]", "data-x=ABC"), None);
    // An empty value matches nothing for the substring forms.
    assert_eq!(m("[data-x^='']", "data-x=abc"), None);
}

#[test]
fn structural_pseudo_classes() {
    let list = |sel: &str, n: usize| {
        let items: String = (0..n).map(|i| format!("<li>{i}</li>")).collect();
        reds(&format!("<style>{sel}{{color:#f00}}</style><ul>{items}</ul>"))
    };
    assert_eq!(list("li:first-child", 3), [true, false, false]);
    assert_eq!(list("li:last-child", 3), [false, false, true]);
    assert_eq!(list("li:nth-child(2)", 4), [false, true, false, false]);
    assert_eq!(list("li:nth-child(odd)", 4), [true, false, true, false]);
    assert_eq!(list("li:nth-child(even)", 4), [false, true, false, true]);
    assert_eq!(list("li:nth-child(2n+1)", 5), [true, false, true, false, true]);
    assert_eq!(list("li:nth-child(-n+2)", 4), [true, true, false, false]);
    assert_eq!(list("li:nth-child(n+3)", 4), [false, false, true, true]);
    assert_eq!(list("li:nth-of-type(2)", 3), [false, true, false]);
    assert_eq!(list("li:not(:first-child)", 3), [false, true, true]);
    assert_eq!(list("li:not(:first-child):not(:last-child)", 4), [false, true, true, false]);
    assert_eq!(list("li:not(.x, :last-child)", 3), [true, true, false]);
    assert_eq!(list("li:only-child", 1), [true]);
}

#[test]
fn nth_of_type_counts_the_elements_of_one_name() {
    assert_eq!(reds("<style>p:nth-of-type(2){color:#f00}</style><div><p>a</p><div>b</div><p>c</p></div>"), [false, false, true]);
}

#[test]
fn what_cannot_be_asked_never_matches() {
    for sel in [
        "a:visited",
        "a:hover",
        "a:focus",
        "a:active",
        "p::before",
        "p:after",
        "p::first-line",
        "a:visited:visited",
        // Not understood: dropped.
        "p:nth-child(",
        "p:is(.a)",
        "p:has(b)",
        "p:unknown",
        "svg|p",
        "p >",
        "> p",
        "",
        ",",
    ] {
        let html = format!("<style>{sel}{{color:#f00}}</style><a href=x>x</a><p>y</p>");
        assert!(reds(&html).iter().all(|red| !red), "{sel}");
    }
    // `:link` is a link with an address.
    assert_eq!(colour("<style>a:link{color:#f00}</style><a href='https://example.org'>x</a>"), RED);
    assert_eq!(colour("<style>a:link{color:#f00}</style><a name=x>x</a>"), None);
}

#[test]
fn a_list_of_selectors_keeps_those_it_understands() {
    assert_eq!(colour("<style>a:hover, p:unknown, p{color:#f00}</style><p>x</p>"), RED);
}

#[test]
fn only_the_whitelist_is_kept() {
    let blocks = parse(
        "<style>p{color:#f00;behavior:url(x.htc);content:'x';position:absolute;background-image:url(https://t.example/p.gif);\
         font-family:url(x);margin:url(a)}</style><p>x</p>",
    );
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].text(), "x");
    assert_eq!(blocks[0].spans[0].color, RED);
    assert_eq!(colour("<style>p{background:#fff url(https://t.example/p.gif)}</style><p>x</p>"), None);
}

#[test]
fn imports_fonts_and_media_do_nothing() {
    let sheet = "@charset \"utf-8\"; @import url(https://t.example/a.css); @import 'b.css';\
        @font-face{font-family:x;src:url(https://t.example/f.woff)}\
        @media (max-width:600px){p{color:#00f}}\
        @keyframes k{from{color:#00f}to{color:#00f}}\
        p{color:#f00}";
    assert_eq!(colour(&format!("<style>{sheet}</style><p>x</p>")), RED);
    assert_eq!(colour("<style>@media screen{p{color:#00f}}</style><p>x</p>"), None);
    assert_eq!(colour("<style media='print'>p{color:#00f}</style><p>x</p>"), None);
    assert_eq!(colour("<style media='(max-width:600px)'>p{color:#00f}</style><p>x</p>"), None);
    assert_eq!(colour("<style media='screen'>p{color:#f00}</style><p>x</p>"), RED);
    assert_eq!(colour("<style type='text/less'>p{color:#f00}</style><p>x</p>"), None);
}

#[test]
fn comments_markers_and_junk_do_not_stop_the_rest() {
    assert_eq!(colour("<style><!-- p{color:#f00} --></style><p>x</p>"), RED);
    assert_eq!(colour("<style>/* a { */ p{/* color:#00f; */color:#f00}</style><p>x</p>"), RED);
    assert_eq!(colour("<style>p{color:#00f;;;color}p{color:#f00}</style><p>x</p>"), RED);
    assert_eq!(colour("<style>p{color:#f00</style><p>x</p>"), RED);
    assert_eq!(colour("<style>p{color:'#00f;'}p{color:#f00}</style><p>x</p>"), RED);
    // A nested rule is skipped whole.
    assert_eq!(colour("<style>p{color:#f00;b{color:#00f}}</style><p>x</p>"), RED);
}

#[test]
fn what_the_sender_hid_with_a_rule_is_not_shown() {
    for hide in ["display:none", "visibility:hidden", "max-height:0;overflow:hidden", "max-height:0", "opacity:0", "height:0;overflow:hidden"] {
        let html = format!("<style>.pre{{{hide}}}</style><div class=pre>Secret preheader</div><p>Hello</p>");
        assert_eq!(texts(&html), ["Hello"], "{hide}");
        assert!(!to_text(&html).contains("Secret"), "{hide}");
    }
    // Hidden by a rule and shown again by the style on the element, as in CSS.
    assert_eq!(texts("<style>.x{display:none}</style><div class=x style='display:block'>shown</div>"), ["shown"]);
    assert_eq!(texts("<style>.x{display:none}.x{display:block}</style><div class=x>shown</div>"), ["shown"]);
    assert_eq!(texts("<style>.x{display:none}#y{display:block}</style><div class=x id=y>shown</div>"), ["shown"]);
    // A hidden element hides what is inside it.
    assert_eq!(texts("<style>.x{display:none}</style><div class=x><p>deep</p></div><p>kept</p>"), ["kept"]);
    // The rule mobile copy is hidden with.
    assert_eq!(texts("<style>.mobile{display:none !important}</style><p class=mobile>m</p><p>d</p>"), ["d"]);
    // Only a matching rule hides.
    assert_eq!(texts("<style>.pre{display:none}</style><div class=other>kept</div>"), ["kept"]);
}

#[test]
fn inline_hiding_is_as_before() {
    assert_eq!(texts("<div style='display:none'>x</div><p>y</p>"), ["y"]);
    assert_eq!(texts("<div style='display:none;display:block'>x</div>"), ["x"]);
    assert_eq!(texts("<div style='visibility:hidden'>x</div><p>y</p>"), ["y"]);
}

#[test]
fn a_size_of_nothing_is_taken_at_its_word_once_the_sheet_is_read() {
    // No sheet: an element with a class is not held to the nothing around it.
    assert!(texts("<div style='font-size:0'><p class=big>shown</p></div>").iter().any(|t| t == "shown"));
    // A sheet that gives the class a size, shown; one that does not, hidden as the size says.
    assert!(texts("<style>.big{font-size:16px}</style><div style='font-size:0'><p class=big>shown</p></div>").iter().any(|t| t == "shown"));
    assert!(!texts("<style>.other{font-size:16px}</style><div style='font-size:0'><p class=big>gone</p></div>").iter().any(|t| t == "gone"));
}

#[test]
fn the_sheet_gives_the_same_facts_a_style_does() {
    let blocks = parse(
        "<style>.t{width:300px;margin:0 auto;background-color:#eee} .c{vertical-align:middle;padding:4px 8px;border:1px solid #ccc} \
         .b{font-weight:bold;font-style:italic;text-decoration:underline;font-size:20px;text-align:center}</style>\
         <table class=t><tr><td class=c>cell</td></tr></table><p class=b>big</p>",
    );
    let styled = parse(
        "<table style='width:300px;margin:0 auto;background-color:#eee'><tr><td style='vertical-align:middle;padding:4px 8px;border:1px solid #ccc'>cell</td></tr></table>\
         <p style='font-weight:bold;font-style:italic;text-decoration:underline;font-size:20px;text-align:center'>big</p>",
    );
    assert_eq!(blocks, styled);
}

#[test]
fn inheritance_is_that_of_the_blocks() {
    let blocks = parse("<style>.w{color:#f00}</style><div class=w><p>a</p><p style='color:#00f'>b</p></div>");
    assert_eq!(blocks[0].spans[0].color, RED);
    assert_eq!(blocks[1].spans[0].color, BLUE);
}

#[test]
fn caps() {
    // A selector of 17 parts is dropped; 16 is kept.
    let deep = |n: usize| format!("{} p", vec!["div"; n - 1].join(" "));
    assert!(selector(&deep(16)).is_some());
    assert!(selector(&deep(17)).is_none());
    // Rules.
    let many = "a{color:red}".repeat(MAX_RULES + 500).chars().take(MAX_SHEET).collect::<String>();
    assert!(many.len() > MAX_RULES * 11);
    assert_eq!(rules(&many).len(), MAX_RULES);
    let commas = format!("{}{{color:#f00}}", vec!["a"; 20_000].join(","));
    assert_eq!(rules(&commas).len(), MAX_RULES);
    // Sheet text: a rule after 64 KB is not read.
    let late = format!("{}p{{color:#f00}}", " ".repeat(MAX_SHEET));
    assert!(rules(&late).is_empty());
    assert_eq!(colour(&format!("<style>{late}</style><p>x</p>")), None);
    // ...and neither is a second sheet that starts after them.
    let first = format!("p{{margin:0}}{}", " ".repeat(MAX_SHEET));
    assert_eq!(colour(&format!("<style>{first}</style><style>p{{color:#f00}}</style><p>x</p>")), None);
}

#[test]
fn declarations_per_element_are_capped_and_the_style_attribute_survives() {
    let many: String = (0..200).map(|i| format!("margin-left:{i}px;")).collect::<String>() + "color:#f00;";
    assert_eq!(colour(&format!("<style>p{{{many}}}</style><p>x</p>")), RED);
    let long = "x".repeat(900);
    let many: String = (0..30).map(|i| format!("font-family:a{long}{i};")).collect();
    let html = format!("<style>p{{{many}}}</style><p style='color:#00f'>x</p>");
    assert_eq!(colour(&html), BLUE);
}

#[test]
fn matching_work_is_bounded_per_element_and_per_message() {
    // Selectors that look at every ancestor and every sibling and never match.
    let nest = "<div>".repeat(300);
    let sheet = (0..200).map(|i| format!("div div div div div div .x{i} + .y ~ .z{{color:#f00}}")).collect::<String>();
    let sheet = sheet + &(0..200).map(|i| format!("div div div div div div div div .q{i}{{color:#f00}}")).collect::<String>();
    let html = format!("<style>{sheet}</style>{nest}<p class='x1 q1'>hostile</p>");
    let Some(mut s) = Sheet::read(&html) else { panic!("a sheet") };
    let mut before = s.work();
    for ordinal in 0..320 {
        s.style_for(ordinal);
        assert!(s.work() - before <= MAX_STEPS_PER_ELEMENT + MAX_CONSIDERED as u32, "element {ordinal}");
        before = s.work();
    }
    assert!(s.work() <= MAX_STEPS);
}

/// A message of about 1 MB written like a newsletter builder does: a long style sheet, nested
/// tables, a class on nearly everything.
fn big_message() -> String {
    let mut sheet = String::from("body{margin:0}.w{width:600px}a{color:#036}");
    for i in 0..12 {
        sheet.push_str(&format!(
            "table.t{i} td.c{i}{{padding:4px}} .row{i} > td + td{{color:#333}} div.s{i}:first-child p{{margin:0}} .k{i}{{font-size:14px}} \
             [data-k=\"{i}\"] span:not(.n){{color:#555}}\n"
        ));
    }
    let mut html = format!("<html><head><style>{sheet}</style></head><body>");
    let mut i = 0;
    while html.len() < 1_000_000 {
        let k = i % 12;
        html.push_str(&format!(
            "<table class='w t{k}' width='600'><tr class='row{k}'><td class='c{k}'><div class='s{k} k{k}'><p>Line {i}</p><span data-k='{k}'>a</span></div></td>\
             <td class='c{k}'><a href='https://example.org/{i}'>link</a></td></tr></table>"
        ));
        i += 1;
    }
    html
}

#[test]
fn a_megabyte_of_mail_is_styled_within_the_budget() {
    let html = big_message();
    assert!(html.len() >= 1_000_000);
    let started = std::time::Instant::now();
    let Some(mut sheet) = Sheet::read(&html) else { panic!("no sheet") };
    let elements = sheet.dom.len();
    assert!(elements > 10_000);
    let styled = (0..elements - 1).filter(|n| sheet.style_for(*n).is_some()).count();
    let took = started.elapsed();
    assert!(styled > 1_000, "{styled}");
    // The work is counted and bounded for the message; the time bound is generous (a loaded CI
    // machine, a build without optimisation).
    assert!(sheet.work() < MAX_STEPS, "all of it was styled within the budget: {}", sheet.work());
    let last = elements - 2;
    assert!(sheet.style_for(last).is_some() || sheet.work() < MAX_STEPS);
    assert!(took.as_millis() < 5_000, "{took:?}");
    println!("styling {elements} elements of {} bytes took {took:?}, {} steps", html.len(), sheet.work());
}

#[test]
fn a_message_that_wears_out_its_budget_is_styled_as_far_as_it_goes() {
    // 4,000 rules keyed by one tag, on a message full of it: the work stops at the cap.
    // (400 steps an element, so 5,000 elements spend it.)
    let sheet: String = (0..MAX_RULES).map(|i| format!("p[data-{i}]{{color:#f00}}")).collect();
    let html = format!("<style>{sheet}</style>{}", "<p>x</p>".repeat(20_000));
    let Some(mut s) = Sheet::read(&html) else { panic!("a sheet") };
    let started = std::time::Instant::now();
    for n in 0..20_000 {
        s.style_for(n);
    }
    assert!(s.work() <= MAX_STEPS && s.work() + MAX_STEPS_PER_ELEMENT > MAX_STEPS, "{}", s.work());
    assert!(started.elapsed().as_millis() < 5_000);
    // Once spent, the elements after are not styled and nothing fails.
    assert!(s.style_for(19_999).is_none());
}

#[test]
fn the_whole_megabyte_becomes_blocks() {
    let blocks = parse(&big_message());
    assert!(blocks.len() > 100);
}

#[test]
fn hostile_sheets_do_not_panic() {
    for sheet in [
        "{".repeat(100_000),
        "}".repeat(100_000),
        "(".repeat(100_000),
        "[".repeat(100_000),
        "@".repeat(100_000),
        "/*".repeat(100_000),
        "p{".repeat(30_000),
        "a,".repeat(30_000),
        ":not(".repeat(30_000),
        "p:nth-child(".repeat(5_000),
        "\\".repeat(60_000),
        "é{é:é}".repeat(5_000),
        "'".repeat(60_000),
        "p{color:\"".repeat(5_000),
        "a>".repeat(30_000),
        format!("{}{{color:red}}", "a ".repeat(30_000)),
        "p:nth-child(99999999999999999999999n+1){color:red}".to_string(),
        "p:nth-child(9223372036854775807n+9223372036854775807){color:red}".to_string(),
        "[a=\"".repeat(10_000),
    ] {
        let html = format!("<style>{sheet}</style><div><p>x</p></div>");
        let _ = parse(&html);
        let _ = rules(&sheet);
    }
}

#[test]
fn deep_and_wide_trees_do_not_panic() {
    let deep = format!("<style>div div p, div>div>p{{color:#f00}}</style>{}<p>x</p>", "<div>".repeat(5_000));
    let _ = parse(&deep);
    let wide = format!("<style>.a ~ .b, .a + .b, .a:last-child{{color:#f00}}</style><div>{}</div>", "<span class=a></span>".repeat(20_000));
    let _ = parse(&wide);
}

#[test]
fn the_tree_closes_what_html_closes() {
    let tree = |html: &str| {
        let mut t = TreeBuilder::new();
        for token in Tokenizer::new(html) {
            match token {
                Token::Open(tag) => t.open(&tag),
                Token::Close(name) => t.close(&name),
                Token::Text(_) => {}
            }
        }
        t.els.iter().skip(1).map(|e| (e.name.clone(), e.parent, e.index)).collect::<Vec<_>>()
    };
    assert_eq!(tree("<ul><li>a<li>b</ul>"), [("ul".into(), 0, 1), ("li".into(), 1, 1), ("li".into(), 1, 2)]);
    assert_eq!(tree("<p>a<p>b"), [("p".into(), 0, 1), ("p".into(), 0, 2)]);
    assert_eq!(tree("<table><tr><td>a<td>b</table>").iter().filter(|e| e.0 == "td").map(|e| e.1).collect::<Vec<_>>(), [2, 2]);
    // A stray end tag closes nothing; a void element has no content.
    assert_eq!(tree("<div></span><br><i>")[2], ("i".into(), 1, 2));
}

proptest::proptest! {
    #[test]
    fn any_bytes_read_as_a_sheet(bytes in proptest::collection::vec(proptest::num::u8::ANY, 0..400)) {
        let text = String::from_utf8_lossy(&bytes);
        let _ = rules(&text);
        let html = format!("<style>{text}</style><div class=a id=b><p>x</p></div>");
        let _ = parse(&html);
        let _ = to_text(&html);
    }

    #[test]
    fn any_css_like_text_reads(sheet in "[a-z.#:>+~*\\[\\]()=\"'{};,!@/\\\\ \\n0-9n-]{0,200}") {
        let _ = rules(&sheet);
        let html = format!("<style>{sheet}</style><div class=a id=b><p class=a>x</p><p>y</p></div>");
        let blocks = parse(&html);
        proptest::prop_assert!(blocks.len() <= crate::MAX_BLOCKS + 2);
    }
}
