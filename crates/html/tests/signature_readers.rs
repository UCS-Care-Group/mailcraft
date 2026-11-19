// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The readers of a signature's HTML take anything (AGENTS.md › Never crash): arbitrary text and
//! HTML-shaped text never make the email-compatibility checker, the reader of the pictures'
//! sources or the writer that scopes a signature's style sheets panic, and what each answers
//! stays inside the text it read: ranges on character boundaries and in order, sources that are
//! the text's own bytes, and a body that is the signature in its wrapper. The fuzz target
//! `signature_html` reads the same way.

use mailcraft_html::compat::{MAX_FINDINGS, MAX_FIX_BYTES, check};
use mailcraft_html::{AuthoredScope, MAX_IMAGE_SOURCES, authored_fragment_with_rebound_cids, image_sources, unescape};
use proptest::prelude::*;

/// One piece of a signature: tags, attributes, declarations, sheets, addresses and odd bytes.
fn piece() -> impl Strategy<Value = String> {
    prop_oneof![
        3 => "[a-z ]{0,6}",
        2 => prop::sample::select(vec![
            "<img src=\"", "<img src='", "<img src=", "<img style=\"", "\">", "'>", ">", "\"", "<style>", "</style>",
            "<style media=\"(max-width:600px)\">", "<table style=\"", "<p style=\"", "<td style=\"", "<iframe src=x>", "</iframe>",
            "<form>", "</form>", "<input>", "<embed>", "<video>", "</video>", "<object>", "</object>", "<template>", "</template>",
            "<base href=x>", "<!--", "-->", "<textarea>", "</textarea>", "<body class=b>", "</p>", "<",
        ])
        .prop_map(str::to_string),
        2 => prop::sample::select(vec![
            "width:10px;", "height:4px;", "width:5em;", "--x:red;", "color:var(--x);", "var(--x)", "var(--x, blue)", "rgb(1 2 3)",
            "hsl(1 2% 3%)", "display:flex;", "display:grid;", "float:left;", "position:absolute;", "max-width:1px;",
            "border-radius:2px;", "box-shadow:1px 1px red;", "background:url(x.png);", "background:linear-gradient(red,blue);",
            "font-family:A;", "1rem", "2vw", "content:'x'", "p::before{content:'x'}", ".a td{color:red}", ":root{--x:blue}",
            "table{max-width:600px}", "@media (max-width:600px){", "}", "{", "@import 'x';", "url(x)", "!important", ";", ":",
        ])
        .prop_map(str::to_string),
        1 => prop::sample::select(vec![
            "cid:a@example.org", "https://example.org/a.png", "data:image/png;base64,AA", "data:image/svg+xml,x", "x.svg", "y.webp",
            "&amp;", "&#x3c;", "&quot;", "\u{e9}", "\u{1f600}", "\u{0}", "\r", "\\", "mc-check", "mc-sig-test",
        ])
        .prop_map(str::to_string),
        1 => any::<char>().prop_map(String::from),
        // A variable whose value is about as long as a fix may copy (MAX_FIX_BYTES), shorter or
        // longer: used in the same `style` attribute, or defined on its own.
        1 => (200usize..400).prop_map(|n| format!("<p style=\"--x:{};color:var(--x)\">", "a".repeat(n))),
        1 => (200usize..400).prop_map(|n| format!("--x:{};", "a".repeat(n))),
    ]
}

/// Arbitrary text, or a document made of [`piece`]s.
fn document() -> impl Strategy<Value = String> {
    prop_oneof![any::<String>(), prop::collection::vec(piece(), 0..48).prop_map(|pieces| pieces.concat())]
}

fn findings_stay_in_the_text(html: &str) -> Result<(), TestCaseError> {
    let Ok(found) = check(html) else { return Ok(()) };
    prop_assert!(found.len() <= MAX_FINDINGS);
    for pair in found.windows(2) {
        if let [a, b] = pair {
            prop_assert!((a.range.start, a.range.end) <= (b.range.start, b.range.end), "out of order: {a:?} then {b:?}");
        }
    }
    for f in &found {
        prop_assert!(html.get(f.range.clone()).is_some(), "{f:?} is not a range of {html:?}");
        prop_assert_eq!(f.severity, f.rule.severity());
        if let Some(fix) = &f.fix {
            prop_assert!(html.get(fix.range.clone()).is_some(), "{fix:?} is not a range of {html:?}");
            prop_assert!(fix.replacement.len() <= MAX_FIX_BYTES, "{fix:?} is longer than a fix may be");
        }
    }
    Ok(())
}

fn sources_are_the_texts_own(html: &str) -> Result<(), TestCaseError> {
    let Ok(sources) = image_sources(html) else { return Ok(()) };
    prop_assert!(sources.len() <= MAX_IMAGE_SOURCES);
    let mut after = 0usize;
    for source in &sources {
        let written = html.get(source.value_range.clone());
        prop_assert!(written.is_some(), "{source:?} is not a range of {html:?}");
        prop_assert!(source.value_range.start >= after, "{source:?} is before the source ahead of it");
        after = source.value_range.end;
        prop_assert_eq!(unescape(written.unwrap_or_default()), source.src.clone());
    }
    Ok(())
}

fn written_signature_is_wrapped(html: &str) -> Result<(), TestCaseError> {
    let Ok(scope) = AuthoredScope::new("mc-sig-test") else { return Err(TestCaseError::fail("no scope")) };
    let Ok(fragment) = authored_fragment_with_rebound_cids(html, &scope, &mut |cid: &str| Some(format!("{cid}.rebound"))) else { return Ok(()) };
    prop_assert!(fragment.body.starts_with("<div class=\"mc-sig-test"), "{:?}", fragment.body);
    prop_assert!(fragment.body.ends_with("</div>"), "{:?}", fragment.body);
    let head = fragment.head.to_ascii_lowercase();
    prop_assert!(head.is_empty() || (head.starts_with("<style") && head.ends_with("</style>")), "{head:?}");
    for refused in ["@import", "url(", "<script", "\\"] {
        prop_assert!(!head.contains(refused), "{refused:?} written in {head:?}");
    }
    Ok(())
}

/// The seeds of the fuzz target `signature_html` (`fuzz/seeds/signature_html/`): small invented
/// signatures that reach each reader's rules, so that its runs start from them.
const FUZZ_SEEDS: [(&str, &str); 5] = [
    (
        "table-sheet.html",
        concat!(
            "<style>table.sig td{padding:0 8px;font-family:Arial,sans-serif}",
            "@media (max-width:600px){table.sig td{display:block}}</style>",
            "<table class=\"sig\"><tr><td><img src=\"cid:logo@example.org\" width=\"64\" alt=\"Logo\"></td>",
            "<td><b>Ada Example</b><br><a href=\"https://example.org\">example.org</a></td></tr></table>",
        ),
    ),
    (
        "variables.html",
        concat!(
            "<style>:root{--brand:#0a6e4f}</style>",
            "<p style=\"--line:1px solid #ccc;color:var(--brand);border-top:var(--line)\">Ada Example</p>",
            "<p style=\"--long:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa;",
            "font-family:var(--long)\">Example Ltd</p>",
        ),
    ),
    (
        "pictures.html",
        concat!(
            "<img src=\"https://example.org/logo.svg\" alt=\"A\"><img src=\"https://example.org/b.webp\" alt=\"B\">",
            "<img src=\"data:image/png;base64,AA\" alt=\"C\"><img src=\"cid:d@example.org\" style=\"width:120px;height:40px\" alt=\"D\">",
            "<img src=\"https://example.org/e.png?a=1&amp;b=2\" alt=\"E\">",
        ),
    ),
    (
        "elements.html",
        concat!(
            "<form><input type=\"text\"><button>Send</button></form><iframe src=\"https://example.org/\"></iframe>",
            "<video src=\"https://example.org/v.mp4\"></video><embed src=\"x\"><object data=\"y\"></object>",
            "<template><style>@import \"a.css\";</style></template><p>Ada Example</p>",
        ),
    ),
    (
        "css-warnings.html",
        concat!(
            "<style>p::before{content:\"x\"} .a{display:flex;float:left} .b{display:grid;position:absolute}</style>",
            "<p class=\"a b\" style=\"color:rgb(1 2 3);background:linear-gradient(red,blue);border-radius:4px;",
            "box-shadow:1px 1px #000;max-width:600px;font-size:1rem;width:50vw;font-family:Example Sans\">Ada</p>",
        ),
    ),
];

/// Each fuzz seed is checked, and holds what the readers answer as anything does.
#[test]
fn the_fuzz_seeds_are_read_as_anything_is() {
    for (name, html) in FUZZ_SEEDS {
        assert!(check(html).is_ok(), "{name} is checked");
        findings_stay_in_the_text(html).unwrap_or_else(|e| panic!("{name}: {e}"));
        sources_are_the_texts_own(html).unwrap_or_else(|e| panic!("{name}: {e}"));
        written_signature_is_wrapped(html).unwrap_or_else(|e| panic!("{name}: {e}"));
    }
}

/// Writes the seeds of the fuzz target `signature_html` (`fuzz/seeds/signature_html/`):
/// `cargo test -p mailcraft-html --test signature_readers -- --ignored write_fuzz_seeds`.
#[test]
#[ignore = "writes files into the repository"]
fn write_fuzz_seeds() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/seeds/signature_html");
    std::fs::create_dir_all(&dir).unwrap();
    for (name, html) in FUZZ_SEEDS {
        std::fs::write(dir.join(name), html).unwrap();
    }
}

proptest! {
    #[test]
    fn the_checker_takes_anything(html in document()) {
        findings_stay_in_the_text(&html)?;
    }

    #[test]
    fn the_picture_reader_takes_anything(html in document()) {
        sources_are_the_texts_own(&html)?;
    }

    #[test]
    fn the_scoping_writer_takes_anything(html in document()) {
        written_signature_is_wrapped(&html)?;
    }
}
