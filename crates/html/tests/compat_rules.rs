// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The email-compatibility checker's second batch of rules ([`mailcraft_html::compat::check`]):
//! pictures written into the HTML as data, form controls, frames and media players, text added by
//! a style rule, CSS variables and colours written with spaces (all errors), and the layout, box,
//! background, unit and font warnings. Each is read from inline `style` attributes and from the
//! declarations of `<style>` rules alike, with an exact byte range into the checked text; where
//! one exact edit puts it right, applying it and checking again leaves that finding gone and every
//! byte outside its range as it was.

use std::ops::Range;

use mailcraft_html::compat::{Finding, Fix, Rule, Severity, check};

/// The bytes of the `n`th (from 0) occurrence of `needle` in `hay`.
fn at(hay: &str, needle: &str, n: usize) -> Range<usize> {
    let start = hay.match_indices(needle).nth(n).unwrap_or_else(|| panic!("the fixture holds {needle:?} {} time(s)", n + 1)).0;
    start..start + needle.len()
}

fn found(rule: Rule, range: Range<usize>, fix: Option<Fix>) -> Finding {
    Finding { rule, severity: rule.severity(), range, fix }
}

fn replace(range: Range<usize>, text: &str) -> Option<Fix> {
    Some(Fix { range, replacement: text.to_string() })
}

/// `html` with `fix` made, checked to change nothing outside the fix's range.
fn apply(html: &str, fix: &Fix) -> String {
    let out = format!("{}{}{}", &html[..fix.range.start], fix.replacement, &html[fix.range.end..]);
    assert_eq!(&out[..fix.range.start], &html[..fix.range.start], "the fix changed text before its range");
    assert_eq!(&out[fix.range.start + fix.replacement.len()..], &html[fix.range.end..], "the fix changed text after its range");
    out
}

/// The findings of `html` of `rule` alone.
fn of(html: &str, rule: Rule) -> Vec<Finding> {
    check(html).unwrap_or_else(|e| panic!("the checker reads {html:?}: {e:?}")).into_iter().filter(|f| f.rule == rule).collect()
}

/// OL-IMG-004: a picture whose source is written into the HTML as a `data:` address is an error
/// on the source itself, whatever its type; there is no one edit that puts it right.
#[test]
fn a_picture_written_in_as_data_is_an_error_on_its_source_without_a_fix() {
    let html = r#"<p>Ada Example</p><img src="data:image/png;base64,iVBORw0KGgo=" width="10" height="10" alt="mark">"#;
    let src = at(html, "data:image/png;base64,iVBORw0KGgo=", 0);
    assert_eq!(check(html).expect("checked"), [found(Rule::DataUriImage, src, None)]);
    // Any letter case, white space round it, an unquoted value.
    let spaced = "<img src=' DATA:image/gif;base64,R0lGODlh ' width=1 height=1>";
    let value = at(spaced, " DATA:image/gif;base64,R0lGODlh ", 0);
    assert_eq!(value.start, 10, "the value starts after the quote");
    assert_eq!(of(spaced, Rule::DataUriImage), [found(Rule::DataUriImage, value, None)]);
    let bare = "<img src=data:image/jpeg;base64,AAAA width=1 height=1>";
    assert_eq!(of(bare, Rule::DataUriImage), [found(Rule::DataUriImage, at(bare, "data:image/jpeg;base64,AAAA", 0), None)]);
}

/// A data SVG is both an SVG picture and one written into the HTML: two findings on one range.
#[test]
fn a_data_svg_picture_is_found_by_both_rules() {
    let html = r#"<img src="data:image/svg+xml;base64,PHN2Zy8+" width="1" height="1">"#;
    let src = at(html, "data:image/svg+xml;base64,PHN2Zy8+", 0);
    assert_eq!(check(html).expect("checked"), [found(Rule::SvgImage, src.clone(), None), found(Rule::DataUriImage, src, None)]);
}

/// Attached pictures, pictures on the web and a `data:` that is only part of an address are not
/// written into the HTML; nor is a `data:` address in another attribute or element.
#[test]
fn attached_and_web_pictures_are_not_written_in() {
    for html in [
        r#"<img src="cid:mark@example.org" width="1" height="1">"#,
        r#"<img src="https://example.org/data:image/png" width="1" height="1">"#,
        r#"<img src="mark.png" alt="data:image/png;base64,AAAA" width="1" height="1">"#,
        r#"<a href="data:text/plain,hi">x</a><p data-x="data:image/png,AA">y</p>"#,
    ] {
        assert_eq!(check(html).expect("checked"), [], "{html}");
    }
}

/// OL-EL-002: a form control is an error on its start tag; its fix takes the whole element out,
/// start tag to end tag, and nothing else.
#[test]
fn a_button_is_found_on_its_start_tag_and_its_fix_takes_the_element_out() {
    let html = r#"<p>Ada</p><button type="button" style="color:#333">Call me</button><p>Example</p>"#;
    let tag = at(html, r#"<button type="button" style="color:#333">"#, 0);
    let element = tag.start..at(html, "</button>", 0).end;
    let all = check(html).expect("checked");
    assert_eq!(all, [found(Rule::FormControl, tag, replace(element, ""))]);
    let fixed = apply(html, all[0].fix.as_ref().expect("a fix"));
    assert_eq!(fixed, "<p>Ada</p><p>Example</p>");
    assert_eq!(check(&fixed).expect("checked"), []);
}

/// A void control is taken out alone; a form holds the owner's own content, so it has no fix.
#[test]
fn an_input_is_taken_out_alone_and_a_form_has_no_fix() {
    let html = r#"<FORM action="https://example.org/s"><p>Ada</p><Input type="text" name="q"></FORM>"#;
    let form = at(html, r#"<FORM action="https://example.org/s">"#, 0);
    let input = at(html, r#"<Input type="text" name="q">"#, 0);
    let all = check(html).expect("checked");
    assert_eq!(all, [found(Rule::FormControl, form.clone(), None), found(Rule::FormControl, input.clone(), replace(input, ""))]);
    let fixed = apply(html, all[1].fix.as_ref().expect("a fix"));
    assert_eq!(fixed, r#"<FORM action="https://example.org/s"><p>Ada</p></FORM>"#);
    assert_eq!(check(&fixed).expect("checked"), [found(Rule::FormControl, form, None)]);
}

/// A select ends at its own end tag past its options; a text area's content is text, even when it
/// looks like markup.
#[test]
fn a_select_and_a_text_area_end_at_their_own_end_tags() {
    let html = "<select name=a><option>One</option><option>Two</option></select><textarea><b>x</b></textarea><p>Ada</p>";
    let select = at(html, "<select name=a>", 0);
    let textarea = at(html, "<textarea>", 0);
    let all = check(html).expect("checked");
    assert_eq!(
        all,
        [
            found(Rule::FormControl, select.clone(), replace(select.start..at(html, "</select>", 0).end, "")),
            found(Rule::FormControl, textarea.clone(), replace(textarea.start..at(html, "</textarea>", 0).end, "")),
        ]
    );
    let once = apply(html, all[1].fix.as_ref().expect("a fix"));
    let twice = apply(&once, check(&once).expect("checked")[0].fix.as_ref().expect("a fix"));
    assert_eq!(twice, "<p>Ada</p>");
    assert_eq!(check(&twice).expect("checked"), []);
}

/// An element nested in one of its own name ends at its own end tag, and the outer one at its.
#[test]
fn nested_objects_each_end_at_their_own_end_tag() {
    let html = r#"<object data="a.swf"><object data="b.swf"></object><p>Fallback</p></object><p>Ada</p>"#;
    let outer = at(html, r#"<object data="a.swf">"#, 0);
    let inner = at(html, r#"<object data="b.swf">"#, 0);
    assert_eq!(
        check(html).expect("checked"),
        [
            found(Rule::EmbeddedContent, outer.clone(), replace(outer.start..at(html, "</object>", 1).end, "")),
            found(Rule::EmbeddedContent, inner.clone(), replace(inner.start..at(html, "</object>", 0).end, "")),
        ]
    );
}

/// An embed is void; video and audio players end at their end tags; one the document never
/// closes is found without a fix, since where it ends is not known.
#[test]
fn embeds_and_players_are_found_and_taken_out() {
    let html = r#"<embed src="a.swf"><video src="a.mp4" controls><source src="b.mp4">Your mail can't play it</video><p>Ada</p>"#;
    let embed = at(html, r#"<embed src="a.swf">"#, 0);
    let video = at(html, r#"<video src="a.mp4" controls>"#, 0);
    let all = check(html).expect("checked");
    assert_eq!(
        all,
        [
            found(Rule::EmbeddedContent, embed.clone(), replace(embed, "")),
            found(Rule::MediaPlayer, video.clone(), replace(video.start..at(html, "</video>", 0).end, "")),
        ]
    );
    let fixed = apply(&apply(html, all[1].fix.as_ref().expect("a fix")), all[0].fix.as_ref().expect("a fix"));
    assert_eq!(fixed, "<p>Ada</p>");
    assert_eq!(check(&fixed).expect("checked"), []);

    let open = r#"<p>Ada</p><audio src="a.mp3"><p>Example</p>"#;
    assert_eq!(check(open).expect("checked"), [found(Rule::MediaPlayer, at(open, r#"<audio src="a.mp3">"#, 0), None)]);
}

/// A frame is found although the reader never gives it as a tag (it skips frames whole); its fix
/// takes it out to its end tag.
#[test]
fn a_frame_is_found_and_taken_out() {
    let html = r#"<p>Ada</p><IFRAME src="https://example.org/map" title="map"><p>No frames</p></iframe><p>Example</p>"#;
    let tag = at(html, r#"<IFRAME src="https://example.org/map" title="map">"#, 0);
    let all = check(html).expect("checked");
    assert_eq!(all, [found(Rule::EmbeddedContent, tag.clone(), replace(tag.start..at(html, "</iframe>", 0).end, ""))]);
    let fixed = apply(html, all[0].fix.as_ref().expect("a fix"));
    assert_eq!(fixed, "<p>Ada</p><p>Example</p>");
    assert_eq!(check(&fixed).expect("checked"), []);
    // Two in a row, after a script the reader skips the same way.
    let two = r#"<script>var x = "<iframe>";</script><iframe src=a></iframe><iframe src=b></iframe><p>Ada</p>"#;
    let ranges: Vec<Range<usize>> = of(two, Rule::EmbeddedContent).into_iter().map(|f| f.range).collect();
    assert_eq!(ranges, [at(two, "<iframe src=a>", 0), at(two, "<iframe src=b>", 0)]);
    // One the document never closes: found, with no fix.
    let open = r#"<p>Ada</p><iframe src="a"><p>Example</p>"#;
    assert_eq!(check(open).expect("checked"), [found(Rule::EmbeddedContent, at(open, r#"<iframe src="a">"#, 0), None)]);
}

/// Names that only look like these elements, and the elements as text, in a comment, in a text
/// area's text or in an attribute, are not found.
#[test]
fn look_alikes_of_controls_frames_and_players_are_not_found() {
    for html in [
        r#"<p>Ada <a href="https://example.org/form">form</a> video audio</p><formula></formula><buttons></buttons><videos></videos>"#,
        r#"<!-- <iframe src=x></iframe> <input> --><p title="<iframe src=y></iframe>">Ada</p>"#,
        "<p>&lt;button&gt; and &lt;iframe&gt;</p>",
    ] {
        assert_eq!(check(html).expect("checked"), [], "{html}");
    }
    let text = "<textarea><iframe src=x></iframe><video></textarea>";
    let all = check(text).expect("checked");
    assert_eq!(all.iter().map(|f| f.rule).collect::<Vec<_>>(), [Rule::FormControl], "only the text area: {all:?}");
}

/// OL-SEL-003: a style rule for `::before` or `::after` that adds text is an error on the whole
/// rule, selector to closing brace; it has no fix.
#[test]
fn a_rule_that_adds_text_before_an_element_is_found_on_the_whole_rule() {
    let html = r#"<style>p.phone::before { content: "Tel. "; color: #333 } p { margin: 0 }</style><p class="phone">+1 555 0100</p>"#;
    let rule = at(html, r#"p.phone::before { content: "Tel. "; color: #333 }"#, 0);
    assert_eq!(of(html, Rule::GeneratedContent), [found(Rule::GeneratedContent, rule, None)]);
    // The old single colon, any letter case, inside `@media`, marked important.
    let old = r#"<style>@media screen { A.next:AFTER { CONTENT: '\2192' !important } }</style><a class="next" href="https://example.org">On</a>"#;
    let rule = at(old, r#"A.next:AFTER { CONTENT: '\2192' !important }"#, 0);
    assert_eq!(of(old, Rule::GeneratedContent), [found(Rule::GeneratedContent, rule, None)]);
}

/// A pseudo-element that adds nothing, `content` on an ordinary rule or in a `style` attribute,
/// and other pseudo-classes are not found.
#[test]
fn rules_that_add_no_text_are_not_found() {
    for html in [
        r#"<style>p::after { content: ""; display: block } td:before { content: none } q { content: "x" } a:hover { color: red }</style><p>Ada</p>"#,
        r#"<p style="content: 'x'">Ada</p><style>/* p::before { content: "x" } */ p { color: #333 }</style>"#,
    ] {
        assert_eq!(of(html, Rule::GeneratedContent), [], "{html}");
    }
}

/// OL-DEC-003: a custom property's definition and each `var()` are errors. A `var()` in a `style`
/// attribute whose variable is defined once, at the root, with a plain value, is fixed by putting
/// that value in its place; the definition is left for the owner to take out.
#[test]
fn a_variable_defined_once_at_the_root_is_put_in_place_of_its_use() {
    let html = r#"<style>:root { --brand: #0a6e4f }</style><p style="color: var(--brand); margin: 0">Ada</p>"#;
    let definition = at(html, "--brand: #0a6e4f", 0);
    let usage = at(html, "var(--brand)", 0);
    let all = of(html, Rule::CssVariable);
    assert_eq!(all, [found(Rule::CssVariable, definition.clone(), None), found(Rule::CssVariable, usage.clone(), replace(usage, "#0a6e4f"))]);
    let fixed = apply(html, all[1].fix.as_ref().expect("a fix"));
    assert_eq!(fixed, r#"<style>:root { --brand: #0a6e4f }</style><p style="color: #0a6e4f; margin: 0">Ada</p>"#);
    // The `:root` rule can't be sent with the signature anyway (MC-CSS-001); of this rule only
    // the definition is left.
    assert_eq!(of(&fixed, Rule::CssVariable), [found(Rule::CssVariable, definition, None)]);
}

/// A variable defined in the element's own `style` attribute is put in place the same way, a
/// fallback value and all.
#[test]
fn a_variable_defined_on_the_element_itself_is_put_in_place() {
    let html = r#"<td style="--gap:8px;padding:VAR( --gap , 4px )">Ada</td>"#;
    let usage = at(html, "VAR( --gap , 4px )", 0);
    let all = of(html, Rule::CssVariable);
    assert_eq!(all, [found(Rule::CssVariable, at(html, "--gap:8px", 0), None), found(Rule::CssVariable, usage.clone(), replace(usage, "8px"))]);
    let fixed = apply(html, all[1].fix.as_ref().expect("a fix"));
    assert_eq!(fixed, r#"<td style="--gap:8px;padding:8px">Ada</td>"#);
    assert_eq!(of(&fixed, Rule::CssVariable).len(), 1, "only the definition is left");
}

/// Where the value is not known for certain, or can't be written in place as it is, the `var()`
/// is found without a fix.
#[test]
fn variables_that_cant_be_put_in_place_have_no_fix() {
    for html in [
        // Defined twice.
        r#"<style>:root { --c: #111 } body { --c: #222 }</style><p style="color: var(--c)">Ada</p>"#,
        // Defined for some elements only, or for some windows only.
        r#"<style>.card { --c: #111 }</style><p class="card" style="color: var(--c)">Ada</p>"#,
        r#"<style>@media (max-width: 600px) { :root { --c: #111 } }</style><p style="color: var(--c)">Ada</p>"#,
        // Not defined at all; defined on another element.
        r#"<p style="color: var(--missing, #333)">Ada</p>"#,
        r#"<div style="--c:#111"><p style="color: var(--c)">Ada</p></div>"#,
        // Defined by another variable, or with quotes.
        r#"<style>:root { --c: var(--d); --d: #111 }</style><p style="color: var(--c)">Ada</p>"#,
        r#"<style>:root { --f: "Segoe UI" }</style><p style="font-family: var(--f), sans-serif">Ada</p>"#,
        // Used in a style rule rather than a `style` attribute.
        r#"<style>:root { --c: #111 } p { color: var(--c) }</style><p>Ada</p>"#,
        // Written with a character reference.
        r#"<style>:root { --c: #111 }</style><p style="color: var(--c); font-family: &quot;Segoe UI&quot;, sans-serif">Ada</p>"#,
    ] {
        let all = of(html, Rule::CssVariable);
        assert!(
            all.iter().any(|f| html[f.range.clone()].to_ascii_lowercase().starts_with("var(") || f.range.len() > 40),
            "no var() found: {all:?} in {html}"
        );
        assert!(all.iter().all(|f| f.fix.is_none()), "a fix for {html}: {all:?}");
    }
}

/// Text that only looks like a variable is not one.
#[test]
fn words_like_variables_are_not_found() {
    let html =
        r#"<style>.--x { color: #333 } p { font-family: "var(--x)", serif }</style><p style="color:#333;margin:0">var(--brand) and --brand: 1</p>"#;
    assert_eq!(of(html, Rule::CssVariable), []);
}

/// OL-COL-002: a colour function whose numbers are separated by spaces is an error on the call;
/// three plain numbers in `rgb()` are fixed by separating them with commas.
#[test]
fn a_colour_written_with_spaces_is_rewritten_with_commas() {
    let html = r#"<p style="color: rgb(0 128 0); margin: 0">Ada</p>"#;
    let call = at(html, "rgb(0 128 0)", 0);
    let all = check(html).expect("checked");
    assert_eq!(all, [found(Rule::SpaceSeparatedColour, call.clone(), replace(call, "rgb(0,128,0)"))]);
    let fixed = apply(html, all[0].fix.as_ref().expect("a fix"));
    assert_eq!(fixed, r#"<p style="color: rgb(0,128,0); margin: 0">Ada</p>"#);
    assert_eq!(check(&fixed).expect("checked"), []);
    // In a style rule, in any letter case, with more space.
    let sheet = "<style>td { background-color: RGB( 10  20 30 ) }</style><table><tr><td>Ada</td></tr></table>";
    let call = at(sheet, "RGB( 10  20 30 )", 0);
    let all = of(sheet, Rule::SpaceSeparatedColour);
    assert_eq!(all, [found(Rule::SpaceSeparatedColour, call.clone(), replace(call, "RGB(10,20,30)"))]);
    assert_eq!(of(&apply(sheet, all[0].fix.as_ref().expect("a fix")), Rule::SpaceSeparatedColour), []);
}

/// One with a transparency, percentages, a hue, or `rgba()` is found without a fix.
#[test]
fn colours_with_spaces_that_are_not_three_plain_numbers_have_no_fix() {
    let html = r#"<p style="color: rgb(0 128 0 / 50%); background-color: rgb(0% 50% 0%); border-color: hsl(120 50% 40%) rgba(0 0 0)">Ada</p>"#;
    let calls = ["rgb(0 128 0 / 50%)", "rgb(0% 50% 0%)", "hsl(120 50% 40%)", "rgba(0 0 0)"];
    let expected: Vec<Finding> = calls.iter().map(|c| found(Rule::SpaceSeparatedColour, at(html, c, 0), None)).collect();
    assert_eq!(of(html, Rule::SpaceSeparatedColour), expected);
}

/// Colours written with commas, and the words in a string, are not found.
#[test]
fn colours_with_commas_are_not_found() {
    let html = r#"<p style="color: rgb(0,128,0); background-color: rgba(0, 0, 0, .5); border-color: hsl(120, 50%, 40%); font-family: 'rgb(0 0 0)', serif">rgb(0 128 0)</p>"#;
    assert_eq!(of(html, Rule::SpaceSeparatedColour), []);
}

/// The warnings of a declaration that come from its property and value alone: each on the whole
/// declaration, a warning, with no fix.
fn warned(html: &str, rule: Rule, declarations: &[&str]) {
    let expected: Vec<Finding> = declarations.iter().map(|d| found(rule, at(html, d, 0), None)).collect();
    assert_eq!(of(html, rule), expected, "{}", rule.id());
    assert!(expected.iter().all(|f| f.severity == Severity::Warning), "{} is not a warning", rule.id());
}

/// OL-LAY-001 to OL-LAY-005: layout a classic Windows mail program ignores, in a `style`
/// attribute or a style rule.
#[test]
fn layout_that_is_ignored_is_a_warning_on_its_declaration() {
    let html = concat!(
        r#"<style>.row { DISPLAY: inline-flex } .cols { display: grid !important } td.side { min-width: 120px } table.card { max-width: 600px }</style>"#,
        r#"<div style="display:flex;gap:8px"><img src="a.png" width="1" height="1" style="float:left"><span style="position: absolute; top: 0">x</span></div>"#,
        r#"<div style="max-width:600px">Ada</div><table style="max-width:600px"><tr><td>Example</td></tr></table>"#
    );
    warned(html, Rule::Flexbox, &["DISPLAY: inline-flex", "display:flex"]);
    warned(html, Rule::Grid, &["display: grid !important"]);
    warned(html, Rule::Float, &["float:left"]);
    warned(html, Rule::Positioned, &["position: absolute"]);
    warned(html, Rule::MinMaxWidth, &["min-width: 120px", "max-width:600px"]);
}

/// Layout that is kept, or that sets nothing, is not found.
#[test]
fn layout_that_is_kept_is_not_found() {
    let html = concat!(
        r#"<style>td { display: table-cell } p { position: static; float: none } table { max-width: 600px }</style>"#,
        r#"<div style="display:block;max-width:none"><span style="display:inline-block;position:static">Ada</span></div><table style="max-width:600px"><tr><td style="display:none">x</td></tr></table>"#
    );
    for rule in [Rule::Flexbox, Rule::Grid, Rule::Float, Rule::Positioned, Rule::MinMaxWidth] {
        assert_eq!(of(html, rule), [], "{}", rule.id());
    }
}

/// OL-BOX-007, OL-BOX-008, OL-BG-001, OL-BG-004: rounded corners, shadows, background pictures and
/// gradients.
#[test]
fn corners_shadows_and_backgrounds_are_warnings_on_their_declarations() {
    let html = concat!(
        r#"<style>.card { border-top-left-radius: 6px; box-shadow: 0 1px 2px #999 } .band { background-image: linear-gradient(#fff, #eee) }</style>"#,
        r#"<td style="border-radius:4px;background:#fff url(https://example.org/b.png) no-repeat">Ada</td>"#,
        r#"<td style="background-image:url('https://example.org/c.png');background:radial-gradient(#fff,#000)">Example</td>"#
    );
    warned(html, Rule::RoundedCorners, &["border-top-left-radius: 6px", "border-radius:4px"]);
    warned(html, Rule::BoxShadow, &["box-shadow: 0 1px 2px #999"]);
    warned(
        html,
        Rule::BackgroundImage,
        &["background:#fff url(https://example.org/b.png) no-repeat", "background-image:url('https://example.org/c.png')"],
    );
    warned(html, Rule::Gradient, &["background-image: linear-gradient(#fff, #eee)", "background:radial-gradient(#fff,#000)"]);
}

#[test]
fn square_corners_no_shadow_and_plain_backgrounds_are_not_found() {
    let html = r#"<td style="border-radius:0;box-shadow:none;background:#fff;background-image:none;background-color:#eee">Ada</td><style>p { border-radius: 0px 0 }</style>"#;
    for rule in [Rule::RoundedCorners, Rule::BoxShadow, Rule::BackgroundImage, Rule::Gradient] {
        assert_eq!(of(html, rule), [], "{}", rule.id());
    }
}

/// OL-UNIT-001 and OL-UNIT-002: each size in `rem` or relative to the window, on the number and its
/// unit.
#[test]
fn sizes_in_rem_and_window_units_are_warnings_on_the_size() {
    let html = r#"<style>h1 { font-size: 1.5REM; margin: -.5rem 0 }</style><div style="width: 50vw; height:10VH; padding: 2vmin 1vmax">Ada</div>"#;
    let sizes = |units: &[&str]| units.iter().map(|u| at(html, u, 0)).collect::<Vec<_>>();
    let ranges = |rule| {
        of(html, rule).into_iter().inspect(|f| assert!(f.fix.is_none() && f.severity == Severity::Warning)).map(|f| f.range).collect::<Vec<_>>()
    };
    assert_eq!(ranges(Rule::RemUnit), sizes(&["1.5REM", "-.5rem"]));
    assert_eq!(ranges(Rule::ViewportUnit), sizes(&["50vw", "10VH", "2vmin", "1vmax"]));
}

#[test]
fn other_units_and_words_with_these_letters_are_not_found() {
    let html = r#"<p style="font-size:13px;letter-spacing:.1em;width:50%;font-family:'Lorem 2rem', serif;margin:0 auto">Ada 2rem 10vw</p><img src="a.png" width="1" height="1" style="background:url(https://example.org/x2rem.png)">"#;
    assert_eq!(of(html, Rule::RemUnit), []);
    assert_eq!(of(html, Rule::ViewportUnit), []);
}

/// OL-FNT-002: a `font-family` list that doesn't end with a generic family.
#[test]
fn a_font_list_without_a_generic_family_at_its_end_is_a_warning() {
    let html = r#"<style>p { font-family: "Segoe UI", Tahoma }</style><td style="font-family:Arial, 'sans-serif'">Ada</td><td style="font-family:Georgia">x</td>"#;
    warned(html, Rule::NoGenericFontFamily, &[r#"font-family: "Segoe UI", Tahoma"#, "font-family:Arial, 'sans-serif'", "font-family:Georgia"]);
}

#[test]
fn a_font_list_that_ends_with_a_generic_family_is_not_found() {
    let html = r#"<style>p { font-family: "Segoe UI", Tahoma, SANS-SERIF } code { font-family: Consolas, monospace !important }</style><td style="font-family:Georgia,serif">Ada</td><td style="font-family:inherit">x</td>"#;
    assert_eq!(of(html, Rule::NoGenericFontFamily), []);
}

/// Warnings never refuse a document, and every one in a busy signature is a warning.
#[test]
fn a_document_with_only_warnings_is_checked_and_has_no_error() {
    let html = r#"<table style="border-radius:8px;box-shadow:0 0 4px #ccc"><tr><td style="display:flex;font-size:1rem;font-family:Calibri">Ada</td></tr></table>"#;
    let all = check(html).expect("checked");
    assert_eq!(all.len(), 5, "{all:?}");
    assert!(all.iter().all(|f| f.severity == Severity::Warning && f.fix.is_none()), "{all:?}");
}

#[test]
fn the_new_rules_have_their_rulebook_ids_and_severities() {
    let expected = [
        (Rule::DataUriImage, "OL-IMG-004", Severity::Error),
        (Rule::FormControl, "OL-EL-002", Severity::Error),
        (Rule::EmbeddedContent, "OL-EL-003", Severity::Error),
        (Rule::MediaPlayer, "OL-EL-004", Severity::Error),
        (Rule::GeneratedContent, "OL-SEL-003", Severity::Error),
        (Rule::CssVariable, "OL-DEC-003", Severity::Error),
        (Rule::SpaceSeparatedColour, "OL-COL-002", Severity::Error),
        (Rule::Flexbox, "OL-LAY-001", Severity::Warning),
        (Rule::Grid, "OL-LAY-002", Severity::Warning),
        (Rule::Float, "OL-LAY-003", Severity::Warning),
        (Rule::Positioned, "OL-LAY-004", Severity::Warning),
        (Rule::MinMaxWidth, "OL-LAY-005", Severity::Warning),
        (Rule::RoundedCorners, "OL-BOX-007", Severity::Warning),
        (Rule::BoxShadow, "OL-BOX-008", Severity::Warning),
        (Rule::BackgroundImage, "OL-BG-001", Severity::Warning),
        (Rule::Gradient, "OL-BG-004", Severity::Warning),
        (Rule::RemUnit, "OL-UNIT-001", Severity::Warning),
        (Rule::ViewportUnit, "OL-UNIT-002", Severity::Warning),
        (Rule::NoGenericFontFamily, "OL-FNT-002", Severity::Warning),
        (Rule::StyleNotSendable, "MC-CSS-002", Severity::Error),
    ];
    for (rule, id, severity) in expected {
        assert_eq!(rule.id(), id);
        assert_eq!(rule.severity(), severity, "{id}");
        assert!(Rule::ALL.contains(&rule), "{id} is not in Rule::ALL");
    }
    let mut ids: Vec<&str> = Rule::ALL.iter().map(|r| r.id()).collect();
    assert_eq!(ids.len(), 24, "the four rules of the first part, the nineteen of this one and MC-CSS-002");
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), 24, "two rules share an id");
}
