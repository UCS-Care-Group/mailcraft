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
        s.style_for(ordinal, false);
        assert!(s.work() - before <= MAX_STEPS_PER_ELEMENT + MAX_CONSIDERED as u32, "element {ordinal}");
        before = s.work();
    }
    assert!(s.work() <= s.budget);
}

/// A message of about 1 MB written like a newsletter builder does: a long style sheet, nested
/// tables, a class on nearly everything.
fn big_message() -> String {
    let mut sheet = String::from("body{margin:0}.w{width:600px}a{color:#036}");
    for i in 0..4 {
        sheet.push_str(&format!(
            "table.t{i} td.c{i}{{padding:4px}} .row{i} > td + td{{color:#333}} div.s{i}:first-child p{{margin:0}} .k{i}{{font-size:14px}} \
             [data-k=\"{i}\"] span:not(.n){{color:#555}}\n"
        ));
    }
    let mut html = format!("<html><head><style>{sheet}</style></head><body>");
    let mut i = 0;
    while html.len() < 1_000_000 {
        let k = i % 4;
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
    let styled = (0..elements - 1).filter(|n| sheet.style_for(*n, false).is_some()).count();
    let took = started.elapsed();
    assert!(styled > 1_000, "{styled}");
    // The work is counted and bounded for the message; the time bound is generous (a loaded CI
    // machine, a build without optimisation).
    assert!(sheet.work() < sheet.budget, "all of it was styled within the budget: {}", sheet.work());
    let last = elements - 2;
    assert!(sheet.style_for(last, false).is_some() || sheet.work() < sheet.budget);
    assert!(took.as_millis() < 5_000, "{took:?}");
    println!("styling {elements} elements of {} bytes took {took:?}, {} steps", html.len(), sheet.work());
}

#[test]
fn a_message_that_wears_out_its_budget_is_styled_as_far_as_it_goes() {
    // 4,000 rules keyed by one tag, on a message full of it: the work stops at the cap.
    // (600 steps an element, so about 7,000 elements spend it.)
    let sheet: String = (0..MAX_RULES).map(|i| format!("p[data-{i}]{{color:#f00}}")).collect();
    let html = format!("<style>{sheet}</style>{}", "<p>x</p>".repeat(20_000));
    let Some(mut s) = Sheet::read(&html) else { panic!("a sheet") };
    let started = std::time::Instant::now();
    for n in 0..20_000 {
        s.style_for(n, false);
    }
    assert!(s.work() <= s.budget && s.work() + MAX_STEPS_PER_ELEMENT > s.budget, "{}", s.work());
    assert!(started.elapsed().as_millis() < 5_000);
    // Once spent, the elements after are not styled and nothing fails.
    assert!(s.style_for(19_999, false).is_none());
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

// ---- review of #598 ----

/// Styles every element of `html` and gives the work done and the time it took.
fn styled(html: &str) -> (Sheet, std::time::Duration) {
    let started = std::time::Instant::now();
    let Some(mut sheet) = Sheet::read(html) else { panic!("no sheet") };
    for n in 0..sheet.dom.len().saturating_sub(1) {
        sheet.style_for(n, false);
    }
    (sheet, started.elapsed())
}

#[test]
fn a_not_list_is_capped_and_every_entry_costs_a_step() {
    // Past the cap the rule is dropped: 20,000 entries cost nothing.
    let long = format!("*:not({}){{color:#f00}}", vec![".a"; 20_000].join(","));
    assert!(rules(&long).is_empty());
    let capped = format!("*:not({}){{color:#f00}}", vec![".a"; MAX_NOT_LIST].join(","));
    assert_eq!(rules(&capped).len(), 1);
    // Within it, each entry is charged: an element that none of 32 entries matches costs 33 steps.
    let html = format!("<style>{capped}</style>{}", "<b>x</b>".repeat(1_000));
    let (sheet, _) = styled(&html);
    assert!(sheet.work() >= 1_000 * (MAX_NOT_LIST as u32 + 1), "{}", sheet.work());
    assert!(sheet.work() <= sheet.budget);
}

#[test]
fn the_simple_selectors_of_a_compound_are_charged() {
    let classes: String = (0..30).map(|i| format!(".q{i}")).collect();
    let html =
        format!("<style>*{classes}{{color:#f00}}</style>{}", "<i class='q0 q1 q2 q3 q4 q5 q6 q7 q8 q9 q10 q11 q12 q13 q14 q15'>x</i>".repeat(5_000));
    let (sheet, _) = styled(&html);
    // 16 of the 30 classes are there: 17 simples are tried and the 18th ends it.
    assert!(sheet.work() >= 5_000 * 17, "{}", sheet.work());
}

#[test]
fn a_match_that_ignores_case_lowers_the_values_once() {
    let rules: String = (0..2_000).map(|i| format!("[title*=q{i} i]{{color:#f00}}")).collect();
    let value = "AbCdEfGh".repeat(30);
    let html = format!("<style>{rules}</style>{}", format!("<i title='{value}'>x</i>").repeat(20_000));
    let started = std::time::Instant::now();
    let Some(mut sheet) = Sheet::read(&html) else { panic!("no sheet") };
    assert_eq!(sheet.dom.get(1).map(|e| e.lower.len()), Some(1));
    for n in 0..20_000 {
        sheet.style_for(n, false);
    }
    assert!(sheet.work() <= sheet.budget);
    assert!(started.elapsed().as_secs() < 3, "{:?}", started.elapsed());
    // And it still matches what it says.
    assert_eq!(colour("<style>[title^=abc i]{color:#f00}</style><p title='ABCD'>x</p>"), RED);
    assert_eq!(colour("<style>[title^=abc]{color:#f00}</style><p title='ABCD'>x</p>"), None);
}

#[test]
fn a_stray_end_tag_costs_nothing_and_a_walk_is_short() {
    let walk = |html: &str| {
        let mut t = TreeBuilder::new();
        for token in Tokenizer::new(html) {
            match token {
                Token::Open(tag) => t.open(&tag),
                Token::Close(name) => t.close(&name),
                Token::Text(_) => {}
            }
        }
        t.walked
    };
    assert_eq!(walk(&format!("{}{}", "<i>".repeat(600), "</b>".repeat(450_000))), 0);
    // A name that is open but behind a barrier: no more than a short walk each time.
    let deep = format!("<b><span>{}", "<i>".repeat(500));
    assert!(walk(&format!("{deep}{}", "</b>".repeat(10_000))) <= 10 * 10_001);
    // Implied closes are bounded the same way.
    assert!(walk(&format!("<p>{}", "<div>".repeat(50_000))) <= 10 * 50_001);
}

#[test]
fn elements_that_match_the_same_rules_share_one_look() {
    let wide: String =
        ["margin-top", "margin-left", "margin-bottom", "margin-right", "padding-top", "padding-left", "padding-right", "padding-bottom"]
            .iter()
            .flat_map(|p| (0..5).map(move |i| format!("{p}:{i}px;")))
            .collect();
    let html = format!("<style>*{{{wide}color:#00f}} i{{{}}}</style>{}", wide.replace(';', " !important;"), "<i>x</i>".repeat(100_000));
    let (sheet, took) = styled(&html);
    assert!(sheet.cache.len() <= 4, "{}", sheet.cache.len());
    assert!(took.as_secs() < 3, "{took:?}");
    // The work is the matching and a share of what a new look costs: not a look for each.
    assert!(sheet.work() < sheet.budget, "{}", sheet.work());
}

#[test]
fn a_size_given_by_a_rule_that_was_not_read_does_not_hide_the_text() {
    let wrapped = |sheet: &str| texts(&format!("<style>{sheet}</style><div style='font-size:0'><span class=t>Hello</span></div>"));
    for sheet in [
        "body{margin:0} @media screen{.t{font-size:16px}}",
        ".t:where(span){font-size:16px}",
        ".t:is(span){font-size:16px}",
        ".t:last-of-type{font-size:16px}",
        ".t:nth-last-child(1){font-size:16px}",
        ".t:only-of-type{font-size:16px}",
        ".t:empty{font-size:16px}",
        ".other{font-size:16px}",
    ] {
        assert_eq!(wrapped(sheet), ["Hello"], "{sheet}");
    }
    // A size the sheet did give is the size: nothing is nothing.
    assert!(wrapped(".t{font-size:0}").is_empty());
    assert_eq!(wrapped(".t{font-size:16px}"), ["Hello"]);
    // Without a sheet, as before.
    assert_eq!(texts("<div style='font-size:0'><span class=t>Hello</span></div>"), ["Hello"]);
}

#[test]
fn an_inline_important_beats_a_sheets_important() {
    assert_eq!(colour("<style>p{color:#0f0 !important}</style><p style='color:#f00 !important'>x</p>"), RED);
    assert_eq!(texts("<style>p{display:none !important}</style><p style='display:block !important'>shown</p>"), ["shown"]);
    // The sheet's still beats an inline that isn't.
    assert_eq!(colour("<style>p{color:#f00 !important}</style><p style='color:#00f'>x</p>"), RED);
    assert_eq!(colour("<style>p{color:#00f}</style><p style='color:#f00;font-weight:bold !important'>x</p>"), RED);
    // A `!important` inside a quoted value is no mark.
    assert_eq!(colour("<style>p{color:#f00 !important}</style><p style=\"font-family:'a !important'; color:#00f\">x</p>"), RED);
}

#[test]
fn only_child_is_first_and_last() {
    assert_eq!(texts("<style>span:only-child{display:none}</style><div><span>AAA</span> <span>BBB</span></div>"), ["AAA BBB"]);
    assert_eq!(texts("<style>span:only-child{display:none}</style><div><span>AAA</span></div><p>kept</p>"), ["kept"]);
    assert_eq!(reds("<style>li:only-child{color:#f00}</style><ul><li>a</li></ul><ul><li>b</li><li>c</li></ul>"), [true, false, false]);
}

#[test]
fn what_hides_can_be_taken_back_by_a_later_declaration() {
    assert_eq!(texts("<style>.x{opacity:0}</style><p class=x style='opacity:1'>shown</p>"), ["shown"]);
    assert_eq!(texts("<style>.x{max-width:0}</style><p class=x style='max-width:600px'>shown</p>"), ["shown"]);
    assert_eq!(texts("<style>.x{height:0;overflow:hidden}</style><p class=x style='height:auto'>shown</p>"), ["shown"]);
    assert_eq!(texts("<style>.x{width:0;overflow:hidden}</style><p class=x style='width:100px'>shown</p>"), ["shown"]);
    assert_eq!(texts("<p style='opacity:0;opacity:1'>shown</p>"), ["shown"]);
    assert_eq!(texts("<style>.x{opacity:0}</style><p class=x>gone</p><p>kept</p>"), ["kept"]);
}

#[test]
fn an_overflow_axis_taken_back_does_not_unclip_the_other() {
    // In CSS `overflow-y:auto` changes only y: the box is still clipped across.
    assert_eq!(texts("<p style='height:0;overflow:hidden;overflow-y:auto'>gone</p><p>kept</p>"), ["kept"]);
    assert_eq!(texts("<p style='height:0;overflow:hidden;overflow-x:auto'>gone</p><p>kept</p>"), ["kept"]);
    assert_eq!(texts("<p style='height:0;overflow:hidden;overflow:visible'>shown</p>"), ["shown"]);
    assert_eq!(texts("<p style='height:0;overflow:hidden;overflow-x:auto;overflow-y:auto'>shown</p>"), ["shown"]);
    assert_eq!(texts("<p style='height:0;overflow:hidden auto'>gone</p><p>kept</p>"), ["kept"]);
}

#[test]
fn a_selector_that_is_not_understood_drops_the_whole_rule() {
    assert_eq!(texts("<style>.x, .y:-moz-foo{display:none}</style><p class=x>shown</p>"), ["shown"]);
    assert_eq!(texts("<style>.x, {display:none}</style><p class=x>shown</p>"), ["shown"]);
    assert_eq!(texts("<style>.x, .y{display:none}</style><p class=x>gone</p><p class=y>gone</p><p>kept</p>"), ["kept"]);
    assert_eq!(texts("<style>.x, .y:hover{display:none}</style><p class=x>gone</p><p>kept</p>"), ["kept"]);
}

#[test]
fn what_an_element_takes_from_its_sheet_is_whole_and_leaves_room() {
    // Junk names are not kept, so they crowd nothing out.
    let junk: String = (0..80).map(|i| format!("margin-z{i}:1px;border-q{i}:1px;padding{i}:2px;")).collect();
    assert_eq!(colour(&format!("<style>p{{{junk}color:#f00}}</style><p>x</p>")), RED);
    assert_eq!(colour(&format!("<style>p{{{junk}color:#0f0 !important}}</style><p style='color:#00f'>x</p>")), Some(Rgb(0, 255, 0)));
    // Forty `!important` declarations and an inline colour: the inline still comes before them
    // and the important ones are all there.
    let props = ["margin-top", "margin-left", "margin-bottom", "margin-right", "padding-top", "padding-left", "padding-right", "padding-bottom"];
    let many: String = props.iter().flat_map(|p| (0..5).map(move |i| format!("{p}:{i}px !important;"))).collect();
    let html = format!("<style>p{{{many}color:#f00 !important}}</style><p style='color:#00f'>x</p>");
    assert_eq!(colour(&html), RED);
    // What is kept is whole declarations, the last ones, within the cap.
    let Some(mut sheet) =
        Sheet::read(&format!("<style>p{{{}}}</style><p>x</p>", (0..40).map(|i| format!("font-family:{}{i};", "z".repeat(60))).collect::<String>()))
    else {
        panic!("no sheet")
    };
    let applied = sheet.style_for(0, false).expect("a look");
    assert!(applied.before.len() <= MAX_APPLIED);
    assert!(applied.before.split(';').all(|d| d.starts_with("font-family:") && d.len() > 12), "{}", applied.before);
}

#[test]
fn the_head_ends_and_the_body_follows_it() {
    let m = |sel: &str| colour(&format!("<html><head><style>{sel}{{color:#f00}}</style></head><body><p>x</p></body></html>"));
    assert_eq!(m("html>body>p"), RED);
    assert_eq!(m("head + body > p"), RED);
    assert_eq!(m("head p"), None);
    // A body without an end tag for the head still follows it.
    assert_eq!(colour("<html><head><style>head+body>p{color:#f00}</style><body><p>x</p>"), RED);
}

#[test]
fn the_first_class_and_id_count_and_a_template_has_no_sheet() {
    assert_eq!(colour("<style>.a{color:#f00}.b{color:#00f}</style><p class=a class=b>x</p>"), RED);
    assert_eq!(colour("<style>#a{color:#f00}#b{color:#00f}</style><p id=a id=b>x</p>"), RED);
    assert_eq!(colour("<template><style>p{color:#f00}</style></template><p>x</p>"), None);
    assert_eq!(colour("<template><style>p{color:#00f}</style></template><style>p{color:#f00}</style><p>x</p>"), RED);
}

// ---- round 2 of the review of #598 ----

/// What a hostile message may spend on its look, in steps: matching, and the looks it builds.
fn assert_within_budget(name: &str, html: &str) {
    let started = std::time::Instant::now();
    let Some(mut sheet) = Sheet::read(html) else { panic!("{name}: no sheet") };
    for n in 0..sheet.dom.len().saturating_sub(1) {
        sheet.style_for(n, false);
    }
    let took = started.elapsed();
    assert_eq!(sheet.budget, steps_for(html.len()), "{name}");
    assert!(sheet.work() <= sheet.budget, "{name}: {} of {}", sheet.work(), sheet.budget);
    // A megabyte gets about a million and a half steps, about 45 ms of matching; a message of
    // any size never more than MAX_STEPS.
    assert!(u64::from(sheet.budget) <= (html.len() as u64 * 3 / 2).max(u64::from(MIN_STEPS)).min(u64::from(MAX_STEPS)), "{name}");
    // Each look built is charged what it costs.
    assert!(
        u64::from(sheet.work()) >= u64::from(sheet.builds) * MIN_BUILD_STEPS || sheet.work() == sheet.budget,
        "{name}: {} builds, {} steps",
        sheet.builds,
        sheet.work()
    );
    // (Generous: a loaded machine, a build without optimisation.)
    assert!(took.as_secs() < 4, "{name}: {took:?}");
    println!("{name}: {} bytes, {} of {} steps, {} looks, {took:?}", html.len(), sheet.work(), sheet.budget, sheet.builds);
}

#[test]
fn the_budget_grows_with_the_message_and_is_capped() {
    assert_eq!(steps_for(0), MIN_STEPS);
    assert_eq!(steps_for(1_000_000), 1_500_000);
    assert_eq!(steps_for(10_000_000), MAX_STEPS);
    assert!(steps_for(100_000) < steps_for(200_000));
}

#[test]
fn sixteen_class_elements_under_a_wall_of_not_lists_cost_what_the_budget_says() {
    let rule = |k: usize| {
        let list = (0..32).map(|i| format!(".a.b.c.d.e.f.g.h.i.j.k.l.m.n.o.p.z{k}x{i}")).collect::<Vec<_>>().join(",");
        format!("*:not({list}){{color:#f00}}")
    };
    let sheet: String = (0..20).map(rule).collect();
    let html = format!("<style>{sheet}</style><p>VISIBLE</p>{}", "<i class='a b c d e f g h i j k l m n o p'>x</i>".repeat(20_000));
    assert!(html.len() > 900_000, "{}", html.len());
    assert_within_budget("not walls", &html);
}

#[test]
fn more_looks_than_the_cache_holds_are_charged_for_each() {
    let selectors: String = (0..4_000).map(|i| format!(".c{i}")).collect::<Vec<_>>().join(",");
    let props = ["margin-top", "margin-left", "margin-bottom", "margin-right", "padding-top", "padding-left", "padding-right", "padding-bottom"];
    let decls: String = props.iter().flat_map(|p| (0..5).map(move |i| format!("{p}:{i}.5px;"))).collect();
    let decls = format!("{decls}font-family:{};", "abcdefghij".repeat(5));
    let html = format!(
        "<style>{selectors}{{{decls}}}</style><p>VISIBLE</p>{}",
        (0..40_000).map(|i| format!("<i class=c{}>x</i>", i % 4_000)).collect::<String>()
    );
    assert!(html.len() > 800_000, "{}", html.len());
    assert_within_budget("many looks", &html);
    let Some(mut sheet) = Sheet::read(&html) else { panic!("no sheet") };
    for n in 0..40_000 {
        sheet.style_for(n, false);
    }
    // More distinct looks than the cache holds were built, and each cost what it should.
    assert!(sheet.builds > MAX_CACHED as u32, "{}", sheet.builds);
    assert!(u64::from(sheet.work()) >= u64::from(sheet.builds) * MIN_BUILD_STEPS);
}

#[test]
fn every_hostile_sheet_case_of_the_corpus_stays_in_its_budget() {
    for (name, html) in crate::corpus::hostile().into_iter().filter(|(name, _)| name.starts_with("sheet")) {
        if Sheet::read(&html).is_some() {
            assert_within_budget(name, &html);
        }
    }
}

#[test]
fn an_end_tag_far_up_the_stack_still_closes_what_it_should() {
    for depth in [3, 64, 65, 70, 500] {
        let html = format!("<style>.h p{{display:none}}</style><div class=h>{}x</div><p>shown</p>", "<span>".repeat(depth));
        assert_eq!(texts(&html), ["x", "shown"], "{depth} spans");
        let fonts = format!("<style>.h p{{display:none}}</style><div class=h>{}x</div><p>shown</p>", "<font>".repeat(depth));
        assert!(texts(&fonts).iter().any(|t| t == "shown"), "{depth} fonts");
    }
}

#[test]
fn an_implied_close_far_up_the_stack_still_happens() {
    for depth in [3, 64, 70, 500] {
        // The second item follows the first, however deep the first was left open.
        let html = format!("<style>li+li{{color:#f00}}</style><ul><li>a{}<li>b</ul>", "<b>".repeat(depth));
        assert_eq!(reds(&html).iter().filter(|r| **r).count(), 1, "{depth} open inside an item");
        let html = format!("<style>li:first-child{{color:#f00}}</style><ul><li>a{}<li>b<li>c</ul>", "<i>".repeat(depth));
        assert_eq!(reds(&html).iter().filter(|r| **r).count(), 1, "{depth} open inside an item, first-child");
        // A paragraph is ended by a block, however many inline elements are open in it.
        let html = format!("<style>p+div{{color:#f00}}</style><p>a{}<div>b</div>", "<span>".repeat(depth));
        assert!(reds(&html).iter().any(|r| *r), "{depth} open inside a paragraph");
    }
}

#[test]
fn the_inline_share_stays_at_sixty_four_declarations_with_a_sheet() {
    // 70 declarations: the 65th, `color`, is past the cap, with a sheet as without one.
    let pad: String = (0..64).map(|i| format!("margin-left:{i}px;")).collect();
    let inline = format!("{pad}color:#f00");
    assert_eq!(colour(&format!("<p style='{inline}'>x</p>")), None);
    assert_eq!(colour(&format!("<style>.z{{width:9px}}</style><p style='{inline}'>x</p>")), None);
    assert_eq!(colour(&format!("<style>p{{width:9px}}</style><p style='{inline}'>x</p>")), None);
    assert_eq!(colour("<style>p{width:9px}</style><p style='margin:0;color:#f00'>x</p>"), RED);
}

#[test]
fn important_is_a_bang_white_space_and_the_word_in_any_case_for_both_readers() {
    for mark in ["!important", "! important", "!IMPORTANT", "!  Important", "!\timportant"] {
        assert_eq!(colour(&format!("<style>p{{color:#0f0 !important}}</style><p style='color:#f00 {mark}'>x</p>")), RED, "{mark:?}");
        assert_eq!(colour(&format!("<style>p{{color:#f00 {mark}}}</style><p style='color:#00f'>x</p>")), RED, "{mark:?} in a sheet");
        assert_eq!(crate::style::Css::parse(&format!("color:#f00 {mark}")).color, RED, "{mark:?} in a style");
        assert_eq!(texts(&format!("<p style='display:none {mark}'>gone</p><p>kept</p>")), ["kept"], "{mark:?} hides");
    }
    // Not a mark: the word without the bang, a bang inside the word.
    assert_eq!(crate::style::Css::parse("color:important").color, None);
}
