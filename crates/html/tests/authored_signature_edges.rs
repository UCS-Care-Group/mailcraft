// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Edges of the authored signature writer and of the compatibility fixes it is read with: a
//! `<body>` inside a `<template>` (wherever that template is), `@media` and `media` at the
//! reader's limit, the first `media` and `type` of a sheet past the attribute cap, and `var()`
//! fixes that would cross a comment or an unquoted attribute's end.
//!
//! The documents are invented; nothing here is fetched.
use mailcraft_html::compat::{Rule, check};
use mailcraft_html::{
    AuthoredFragment, AuthoredScope, Block, BlockKind, Env, Rgb, authored_fragment_with_rebound_cids, image_sources, parse_in, quotable,
};
fn write(html: &str) -> Result<AuthoredFragment, mailcraft_html::AuthoredError> {
    authored_fragment_with_rebound_cids(html, &AuthoredScope::new("mc-probe").unwrap(), &mut |x| Some(x.to_string()))
}
fn color(bs: &[Block]) -> Option<Rgb> {
    bs.iter().find_map(|b| match &b.kind {
        BlockKind::Table(t) => t.rows.iter().flat_map(|r| &r.cells).find_map(|c| color(&c.blocks)),
        _ => b.spans.iter().find(|s| s.text.contains("MARKER")).and_then(|s| s.color),
    })
}
fn html(media: &str) -> String {
    format!("<style media=\"{media}\">p{{color:red}}</style><p>MARKER</p>")
}
fn fix(html: &str) -> Option<String> {
    let fs = check(html).unwrap();
    let f = fs.iter().find(|f| f.rule == Rule::CssVariable && html.get(f.range.clone()).is_some_and(|s| s.starts_with("var(")))?;
    let x = f.fix.as_ref()?;
    let mut out = html.to_string();
    out.replace_range(x.range.clone(), &x.replacement);
    Some(out)
}
#[test]
fn a_template_a_screen_sheet_and_plain_var_fixes_keep_their_baseline() {
    let good = "<p>BEFORE</p><template><p>INERT</p></template><p>AFTER</p>";
    let f = write(good).unwrap();
    assert!(f.body.contains("BEFORE") && f.body.contains("AFTER") && !f.body.contains("INERT"));
    assert!(write(&html("screen")).unwrap().head.contains("media=\"screen\""));
    assert_eq!(fix("<p style=\"--brand:red;color:var(--brand,blue)\">MARKER</p>"), Some("<p style=\"--brand:red;color:red\">MARKER</p>".into()));
    assert!(
        fix("<style>body{--font:Times New Roman, serif}</style><p style=\"font-family:var(--font,serif)\">MARKER</p>")
            .unwrap()
            .contains("style=\"font-family:Times New Roman, serif\"")
    );
}
#[test]
fn a_body_inside_a_template_neither_resets_nor_takes_the_wrapper() {
    let f = write("<p>BEFORE</p><template><body class=\"inert\"><p>INERT</p></template><p>AFTER</p>").unwrap();
    println!("OUTPUT {f:?}");
    assert!(f.body.contains("BEFORE") && f.body.contains("AFTER") && !f.body.contains("inert"), "template body lost text or stole wrapper: {f:?}");
}
#[test]
fn an_at_media_query_over_the_readers_limit_is_refused() {
    let q = format!("screen{}and (min-width: 600px)", " ".repeat(507));
    let h = format!("<style>@media {q}{{p{{color:red}}}}</style><p>MARKER</p>");
    let f = write(&h);
    println!("query bytes={} WRITER {f:?}", q.len());
    assert!(f.is_err(), "oversize media accepted");
}
#[test]
fn a_comment_inside_var_gets_no_misbounded_fix() {
    let h = "<p style=\"--brand:red;color:var(--brand,/* ) */blue)\">MARKER</p>";
    let f = fix(h);
    println!("FIX {f:?}");
    assert!(f.is_none() || f.as_deref() == Some("<p style=\"--brand:red;color:red\">MARKER</p>"), "offered fix corrupts CSS: {f:?}");
}
#[test]
fn an_unquoted_style_gets_no_fix_that_crosses_its_boundary() {
    let h = "<style>body{--font:Times New Roman, serif}</style><p style=font-family:var(--font,serif)>MARKER</p>";
    let f = fix(h);
    println!("FIX {f:?}");
    assert!(f.is_none() || !f.unwrap().contains("style=font-family:Times New Roman"), "fix breaks HTML attribute boundary");
}
#[test]
fn an_over_long_media_attribute_never_trims_into_a_wider_query() {
    let q = format!("screen{}and (min-width: 600px)", " ".repeat(507));
    let h = html(&q);
    let f = write(&h);
    let c = color(&parse_in(&h, &Env::for_pane(320.0, false)).blocks);
    println!("query bytes={} WRITER {f:?} reader_narrow={c:?}", q.len());
    assert!(f.is_err() && c.is_none(), "oversize condition became screen");
}
#[test]
fn a_style_type_past_the_attribute_cap_still_decides() {
    let attrs = (0..40).map(|i| format!(" data-a{i}=\"\"")).collect::<String>();
    let h = format!("<style media=\"screen\"{attrs} type=\"text/plain\">p{{color:red}}</style><p>MARKER</p>");
    let f = write(&h);
    println!("WRITER {f:?} reader={:?}", color(&parse_in(&h, &Env::default()).blocks));
    assert!(f.is_err() || f.unwrap().head.is_empty(), "non-CSS style activated");
}
#[test]
fn generated_var_fixes_are_offered_only_where_they_are_exact() {
    let mut bad = Vec::new();
    let mut offered = 0;
    let mut withheld = 0;
    let mut cases = 0;
    for comment in ["", "/* ) */", "/* ( */", "/* (( )) */", "/* ) ( */"] {
        for value in ["red", "Times New Roman, serif"] {
            for quote in ["\"", "'", ""] {
                let h = format!("<style>body{{--x:{value}}}</style><p style={quote}color:var(--x,{comment}blue){quote}>MARKER</p>");
                cases += 1;
                match fix(&h) {
                    None => withheld += 1,
                    Some(out) => {
                        offered += 1;
                        let expected = format!("<style>body{{--x:{value}}}</style><p style={quote}color:{value}{quote}>MARKER</p>");
                        if out != expected || (quote.is_empty() && value.contains(' ')) {
                            bad.push((h, out));
                        }
                    }
                }
            }
        }
    }
    println!("PROPERTY cases={cases} offered={offered} withheld={withheld} mismatches={} {bad:?}", bad.len());
    assert!(offered > 0);
    assert!(cases == 30);
    assert!(bad.is_empty());
}

#[test]
fn a_scope_already_in_the_signature_collides_and_a_distinct_one_does_not() {
    let h = r#"<style>.mc-preview{color:red}</style><p class="mc-preview">MARKER</p>"#;
    let a = authored_fragment_with_rebound_cids(h, &AuthoredScope::new("mc-preview").unwrap(), &mut |x| Some(x.to_string()));
    let b = write(h).unwrap();
    println!("fixed-preview-scope={a:?} distinct-scope={b:?}");
    assert_eq!(a, Err(mailcraft_html::AuthoredError::ScopeCollides));
    assert!(!b.head.is_empty());
}

#[test]
fn generated_style_attributes_decide_by_their_first_media_and_type() {
    let mut violations = Vec::new();
    let (mut accepted, mut refused, mut inert) = (0, 0, 0);
    let mut count = 0;
    for n in [0, 30, 31, 32, 40] {
        let junk = (0..n).map(|i| format!(" data-x{i}=\"\"")).collect::<String>();
        for media in ["screen", "print", ""] {
            for kind in ["text/css", "text/plain"] {
                for type_first in [false, true] {
                    let attrs = if type_first {
                        format!(" type=\"{kind}\" media=\"{media}\"{junk} type=\"text/plain\"")
                    } else {
                        format!(" media=\"{media}\"{junk} type=\"{kind}\" type=\"text/css\"")
                    };
                    let h = format!("<style{attrs}>p{{color:red}}</style><p>MARKER</p>");
                    count += 1;
                    let is_css = kind == "text/css";
                    let expected_color = is_css && media != "print";
                    match write(&h) {
                        Err(_) => refused += 1,
                        Ok(f) => {
                            accepted += 1;
                            if f.head.is_empty() {
                                inert += 1;
                            }
                            if !is_css && !f.head.is_empty() {
                                violations.push(format!("writer: n={n} kind={kind} type_first={type_first} media={media:?}"));
                            }
                        }
                    }
                    let actual = color(&parse_in(&h, &Env::default()).blocks) == Some(Rgb(255, 0, 0));
                    if actual != expected_color {
                        violations.push(format!("reader: n={n} kind={kind} type_first={type_first} media={media:?}"));
                    }
                }
            }
        }
    }
    let junk = (0..40).map(|i| format!(" data-c{i}=\"\"")).collect::<String>();
    let missing_media = format!("<style{junk} media=\"print\">p{{color:red}}</style><p>MARKER</p>");
    count += 1;
    match write(&missing_media) {
        Err(_) => refused += 1,
        Ok(f) => {
            accepted += 1;
            if !f.head.contains("media=\"print\"") {
                violations.push("late media lost".into());
            }
        }
    };
    println!("ATTR_PROPERTY cases={count} accepted={accepted} refused={refused} inert={inert} mismatches={} {violations:?}", violations.len());
    assert!(accepted > 0 && inert > 0);
    assert!(violations.is_empty());
}
#[test]
fn generated_media_queries_apply_exactly_up_to_the_readers_limit() {
    let mut bad = Vec::new();
    let (mut allowed, mut denied) = (0, 0);
    let mut cases = 0;
    for spaces in [1, 470, 480, 483, 484, 485, 486, 487, 488, 489, 500, 506, 507, 508, 512, 1024] {
        let q = format!("screen{}and (min-width: 600px)", " ".repeat(spaces));
        let h = html(&q);
        cases += 1;
        let f = write(&h);
        let narrow = color(&parse_in(&h, &Env::for_pane(320.0, false)).blocks);
        let wide = color(&parse_in(&h, &Env::for_pane(900.0, false)).blocks);
        if q.len() > 512 {
            denied += 1;
            if f.is_ok() || narrow.is_some() || wide.is_some() {
                bad.push((q.len(), f.is_ok(), narrow, wide));
            }
        } else {
            allowed += 1;
            if f.is_err() || narrow.is_some() || wide != Some(Rgb(255, 0, 0)) {
                bad.push((q.len(), f.is_ok(), narrow, wide));
            }
        }
    }
    println!("CAP_PROPERTY cases={cases} under_limit={allowed} over_limit={denied} mismatches={} {bad:?}", bad.len());
    assert!(allowed > 0 && denied > 0);
    assert!(bad.is_empty());
}

#[test]
fn every_offered_var_fix_reads_as_the_intended_style() {
    let (mut offered, mut withheld) = (0, 0);
    for quote in ["\"", "'", ""] {
        for comment in ["", "/* ) */", "/* ( */", "/* ) ( */"] {
            for (property, value) in [("color", "red"), ("font-family", "Times New Roman, serif"), ("font-family", "A=B")] {
                let h = format!("<style>body{{--x:{value}}}</style><p style={quote}{property}:var(--x,{comment}blue){quote}>MARKER</p>");
                assert!(check(&h).unwrap().iter().any(|f| f.rule == Rule::CssVariable));
                match fix(&h) {
                    None => withheld += 1,
                    Some(out) => {
                        offered += 1;
                        let expected = format!("<p style=\"{property}:{value}\">MARKER</p>");
                        assert_eq!(
                            parse_in(&out, &Env::default()).blocks,
                            parse_in(&expected, &Env::default()).blocks,
                            "offered fix crosses an HTML/CSS boundary: {h} -> {out}"
                        );
                    }
                }
            }
        }
    }
    println!("DOWNSTREAM_FIX cases=36 offered={offered} withheld={withheld}");
    assert!(offered >= 3 && withheld >= 1);
}

#[test]
fn at_media_queries_of_511_512_and_513_bytes_meet_the_limit_exactly() {
    for len in [511, 512, 513] {
        for (before, after) in [("", ""), (" ", "  "), ("\t", "\n")] {
            let core = format!("screen{}and (min-width: 600px)", " ".repeat(len - 28));
            assert_eq!(core.len(), len);
            let h = format!("<style>@media {before}{core}{after}{{p{{color:red}}}}</style><p>MARKER</p>");
            let got = write(&h);
            if len > 512 {
                assert_eq!(got, Err(mailcraft_html::AuthoredError::Unsupported));
            } else {
                let f = got.unwrap();
                let out = format!("{}{}", f.head, f.body);
                assert_eq!(color(&parse_in(&out, &Env::for_pane(320.0, false)).blocks), None);
                assert_eq!(color(&parse_in(&out, &Env::for_pane(900.0, false)).blocks), Some(Rgb(255, 0, 0)));
            }
        }
    }
}

#[test]
fn the_first_media_and_type_decide_independently_after_the_attribute_cap() {
    let (mut css, mut inert, mut refused) = (0, 0, 0);
    for n in [0, 31, 32, 40] {
        let junk: String = (0..n).map(|i| format!(" data-z{i}='' ")).collect();
        for media in ["screen", "print"] {
            for kind in ["text/css", "text/plain"] {
                for type_first in [false, true] {
                    let attrs = if type_first {
                        format!("type='{kind}' {junk} media='{media}' media='screen' type='text/css'")
                    } else {
                        format!("{junk} media='{media}' type='{kind}' media='screen' type='text/css'")
                    };
                    let h = format!("<style {attrs}>p{{color:red}}</style><p>MARKER</p>");
                    let expected = if media == "screen" && kind == "text/css" { Some(Rgb(255, 0, 0)) } else { None };
                    assert_eq!(color(&parse_in(&h, &Env::default()).blocks), expected, "first media/type lost or second duplicate won: {h}");
                    match write(&h) {
                        Err(_) => refused += 1,
                        Ok(f) => {
                            if f.head.is_empty() {
                                inert += 1;
                            } else {
                                css += 1;
                            }
                            assert_eq!(color(&parse_in(&format!("{}{}", f.head, f.body), &Env::default()).blocks), expected);
                        }
                    }
                }
            }
        }
    }
    println!("INDEPENDENT_METADATA cases=32 css={css} inert={inert} refused={refused}");
    assert!(css > 0 && inert > 0 && refused > 0);
}

/// The elements the writer leaves out with everything inside them, each as the start and the end
/// of a block that holds a template: hidden ones, the dropped kinds, and hidden cells and items
/// whose row or list ends them.
const DROPPED_WHOLE: &[(&str, &str)] = &[
    ("<div hidden>", "</div>"),
    ("<div style=\"display:none\">", "</div>"),
    ("<p hidden>", "</p>"),
    ("<span style=\"visibility:hidden\">", "</span>"),
    ("<div style=\"max-height:0\">", "</div>"),
    ("<noscript>", "</noscript>"),
    ("<object>", "</object>"),
    ("<applet>", "</applet>"),
    ("<button>", "</button>"),
    ("<select>", "</select>"),
    ("<datalist>", "</datalist>"),
    ("<option>", "</option>"),
    ("<optgroup>", "</optgroup>"),
    ("<frameset>", "</frameset>"),
    ("<svg>", "</svg>"),
    ("<math>", "</math>"),
    ("<head>", "</head>"),
    ("<template>", "</template>"),
    ("<table><tr><td hidden>", "</td></tr></table>"),
    ("<ul><li hidden>", "</li></ul>"),
];

/// The wrapper the writer starts with when no `<body>` gives it classes or attributes.
const PLAIN_WRAPPER: &str = "<div class=\"mc-probe\">";

/// A `<body>` inside a template is inert even when the template is inside an element the writer
/// leaves out whole: it neither drops what came before it nor gives the wrapper its classes and
/// style, and the template's picture stays out. The picture after the block is written, and the
/// lister and the writer agree on both.
#[test]
fn a_body_inside_a_template_in_an_element_left_out_whole_neither_resets_nor_takes_the_wrapper() {
    let (inert, kept) = ("https://pictures.example.net/t.png", "https://pictures.example.net/o.png");
    for (open, close) in DROPPED_WHOLE {
        for template_end in ["</template>", ""] {
            let html = format!(
                "<p>BEFORE</p>{open}<template><body class=\"inert\" style=\"color:red\"><img src=\"{inert}\">{template_end}{close}<p>AFTER</p><img src=\"{kept}\">"
            );
            let f = write(&html).unwrap_or_else(|e| panic!("{html}: {e}"));
            assert!(f.body.starts_with(PLAIN_WRAPPER), "{html}: the template's body took the wrapper: {}", f.body);
            assert!(f.body.contains("BEFORE"), "{html}: the template's body dropped what came before it: {}", f.body);
            assert!(!f.body.contains("inert") && !f.body.contains("color:red"), "{html}: {}", f.body);
            assert!(!f.body.contains(inert), "{html}: the template's picture was written: {}", f.body);
            let listed = image_sources(&html).unwrap_or_else(|e| panic!("{html}: {e}"));
            assert_eq!(listed.iter().map(|s| s.src.as_str()).collect::<Vec<_>>(), [inert, kept], "{html}");
            for s in &listed {
                if s.in_template {
                    assert!(!f.body.contains(s.src.as_str()), "{html}: listed as held by a template, yet written: {}", s.src);
                }
            }
            if template_end.is_empty() {
                continue;
            }
            // The block and its template closed, the rest of the document is the writer's own.
            assert!(f.body.contains("AFTER") && f.body.contains(kept), "{html}: {}", f.body);
            assert!(listed.iter().any(|s| s.src == kept && !s.in_template), "{html}: {listed:?}");
        }
    }
}

/// xorshift64*: the same documents at every run.
struct Rng(u64);

impl Rng {
    fn below(&mut self, n: usize) -> usize {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        usize::try_from(x.wrapping_mul(0x2545_F491_4F6C_DD1D) % u64::try_from(n).unwrap()).unwrap()
    }
}

/// Generated documents of every element left out whole, templates, pictures and a `<body>` that
/// only ever comes inside a template: the writer never lets that body drop the document's opening
/// words or take the wrapper, and every picture the lister says a template holds is one the writer
/// leaves out. Each element is met, and both answers of the lister are given.
#[test]
fn generated_templates_in_elements_left_out_whole_keep_the_writer_and_the_lister_agreeing() {
    const DOCUMENTS: usize = 2_000;
    let mut rng = Rng(0xD1B5_4A32_D192_ED03);
    let mut met = vec![0usize; DROPPED_WHOLE.len()];
    let (mut held, mut written, mut bodies) = (0usize, 0usize, 0usize);
    for n in 0..DOCUMENTS {
        let mut html = String::from("<p>BEFORE</p>");
        let mut pictures = 0usize;
        for _ in 0..14 {
            match rng.below(8) {
                0 | 1 => {
                    let at = rng.below(DROPPED_WHOLE.len());
                    met[at] += 1;
                    html.push_str(DROPPED_WHOLE[at].0);
                }
                2 => html.push_str(DROPPED_WHOLE[rng.below(DROPPED_WHOLE.len())].1),
                3 => {
                    bodies += 1;
                    html.push_str("<template><body class=\"stolen\" style=\"color:red\">");
                }
                4 => html.push_str("</template>"),
                5 | 6 => {
                    pictures += 1;
                    html.push_str(&format!("<img src=\"https://pictures.example.net/g{pictures}.png\">"));
                }
                _ => html.push_str("<p>words</p>"),
            }
        }
        let f = write(&html).unwrap_or_else(|e| panic!("document {n}: {html}: {e}"));
        assert!(
            f.body.starts_with(PLAIN_WRAPPER) && !f.body.contains("stolen"),
            "document {n}: a template's body took the wrapper: {html} -> {}",
            f.body
        );
        assert!(f.body.contains("BEFORE"), "document {n}: a template's body dropped what came before it: {html} -> {}", f.body);
        let listed = image_sources(&html).unwrap_or_else(|e| panic!("document {n}: {html}: {e}"));
        assert_eq!(listed.len(), pictures, "document {n}: every picture is listed: {html}");
        for s in &listed {
            // The address ends in `.png"`, so `g1.png` never matches inside `g11.png`.
            let in_body = f.body.contains(&format!("{}\"", s.src));
            if s.in_template {
                held += 1;
                assert!(!in_body, "document {n}: {} is held by a template, yet written: {html} -> {}", s.src, f.body);
            } else if in_body {
                written += 1;
            }
        }
    }
    assert!(met.iter().all(|m| *m > 20), "every element left out whole is met: {met:?}");
    assert!(held > 100 && written > 100 && bodies > 1_000, "held {held}, written {written}, bodies {bodies}");
}

/// Received mail is quoted by its own route, which the authored route's template count leaves
/// alone: there a `<body>` in a template, whatever holds the template, still ends what came before
/// it, exactly as the previous round's bytes quoted these documents.
#[test]
fn received_quoting_of_a_body_in_a_template_is_unchanged() {
    let recorded = "<img src=\"https://pictures.example.net/t.png\"><p>AFTER</p>";
    for (open, close) in [
        ("<div hidden>", "</div>"),
        ("<noscript>", "</noscript>"),
        ("<div style=\"display:none\">", "</div>"),
        ("<object>", "</object>"),
        ("<svg>", "</svg>"),
        ("", ""),
    ] {
        let html = format!(
            "<p>BEFORE</p>{open}<template><body class=\"inert\" style=\"color:red\"><img src=\"https://pictures.example.net/t.png\"></template>{close}<p>AFTER</p>"
        );
        assert_eq!(quotable(&html), recorded, "{html}");
    }
}
