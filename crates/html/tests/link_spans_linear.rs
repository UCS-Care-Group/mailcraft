// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! One long link cut into many spans by the styles inside it (AU3-01): what the blocks repeat
//! of the address stays a small multiple of the input, every span of a link shares one address,
//! and no address longer than `MAX_LINK` is kept, whatever its scheme.

use std::sync::Arc;

use mailcraft_html::{Block, BlockKind, is_safe_url, parse, parse_text_own};
use mailcraft_link::MAX_LINK;

/// Every block, with those inside table cells.
fn flat(blocks: &[Block]) -> Vec<&Block> {
    let mut out = Vec::new();
    for block in blocks {
        out.push(block);
        if let BlockKind::Table(table) = &block.kind {
            for cell in table.rows.iter().flat_map(|r| &r.cells) {
                out.extend(flat(&cell.blocks));
            }
        }
    }
    out
}

/// The link bytes the blocks name, counted once per span or picture that names them: what
/// writing the blocks out repeats.
fn link_bytes(blocks: &[Block]) -> usize {
    flat(blocks)
        .iter()
        .map(|b| {
            let picture = match &b.kind {
                BlockKind::Image { link, .. } => link.as_deref().map_or(0, str::len),
                _ => 0,
            };
            picture + b.spans.iter().map(|s| s.link.as_deref().map_or(0, str::len)).sum::<usize>()
        })
        .sum()
}

fn serialized(blocks: &[Block]) -> usize {
    serde_json::to_vec(blocks).map_or(usize::MAX, |v| v.len())
}

/// The audit's payload: a `mailto:` address of `href` bytes around `n` alternations of bold and
/// plain text.
fn alternations(href: usize, n: usize) -> String {
    let address = format!("mailto:u@example.org?body={}", "a".repeat(href));
    format!("<p><a href=\"{address}\">{}</a></p>", "<b>x</b>x".repeat(n))
}

/// What the blocks may repeat of the links in `html`.
fn allowed(html: &str) -> usize {
    4 * html.len() + 64 * 1024
}

#[test]
fn styled_spans_inside_a_long_link_do_not_multiply_it() {
    let html = alternations(60 * 1024, 1000);
    let blocks = parse(&html);
    let copied = link_bytes(&blocks);
    assert!(copied <= allowed(&html), "{copied} link bytes from {} of input", html.len());
    let written = serialized(&blocks);
    assert!(written <= 64 * html.len(), "{written} bytes written from {}", html.len());
    // The text is all there, and it still leads where it said.
    let text: String = flat(&blocks).iter().flat_map(|b| &b.spans).map(|s| s.text.as_str()).collect();
    assert_eq!(text, "xx".repeat(1000));
    assert!(flat(&blocks).iter().flat_map(|b| &b.spans).next().is_some_and(|s| s.link.is_some()));
}

#[test]
fn text_after_the_allowance_is_spent_never_takes_an_earlier_link() {
    // Enough spans to spend what the links may repeat, then plain text after the link.
    let html = format!("{}<p>after</p><p><i>tail</i> end</p>", alternations(60 * 1024, 1000));
    let blocks = parse(&html);
    for span in flat(&blocks).iter().flat_map(|b| &b.spans) {
        if ["after", "tail", " end"].contains(&span.text.as_str()) {
            assert!(span.link.is_none(), "{span:?}");
        }
    }
    let after: Vec<&str> = flat(&blocks).iter().flat_map(|b| &b.spans).map(|s| s.text.as_str()).filter(|t| !t.starts_with('x')).collect();
    assert_eq!(after, ["after", "tail", " end"]);
    // Within the link, past the allowance, the text is all there with its own look, unlinked.
    let spans: Vec<_> = flat(&blocks).iter().flat_map(|b| &b.spans).filter(|s| s.text == "x").collect();
    assert!(spans.iter().any(|s| s.link.is_none() && s.bold), "{}", spans.len());
}

#[test]
fn the_spans_of_one_link_share_its_address() {
    let blocks = parse("<p><a href=\"mailto:u@example.org\">a<b>b</b>c<i>d</i></a></p>");
    let links: Vec<&Arc<str>> = flat(&blocks).iter().flat_map(|b| &b.spans).filter_map(|s| s.link.as_ref()).collect();
    assert_eq!(links.len(), 4, "{blocks:?}");
    assert!(links.iter().all(|l| Arc::ptr_eq(l, links[0])), "{links:?}");
}

#[test]
fn a_link_across_many_blocks_and_pictures_stays_bounded() {
    let address = format!("cid:{}@example.org", "c".repeat(60 * 1024));
    for inner in ["<p>x</p>", "<img src=\"cid:p@example.org\" alt=\"x\">", "<div><b>x</b>y</div>"] {
        let html = format!("<a href=\"{address}\">{}</a>", inner.repeat(2000));
        let blocks = parse(&html);
        let copied = link_bytes(&blocks);
        assert!(copied <= allowed(&html), "{inner}: {copied} link bytes from {} of input", html.len());
    }
}

#[test]
fn no_address_over_max_link_is_kept_whatever_its_scheme() {
    for scheme in ["mailto:u@example.org?body=", "tel:+1", "cid:", "https://example.org/"] {
        let long = format!("{scheme}{}", "a".repeat(MAX_LINK));
        assert!(!is_safe_url(&long), "{scheme}");
        let blocks = parse(&format!("<p><a href=\"{long}\">x</a></p>"));
        assert_eq!(link_bytes(&blocks), 0, "{scheme}");
        let fits = format!("{scheme}{}", "a".repeat(MAX_LINK - scheme.len()));
        assert!(is_safe_url(&fits), "{scheme}");
    }
    // The owner's own HTML is held to the same length.
    let long = format!("mailto:u@example.org?body={}", "a".repeat(MAX_LINK));
    let blocks = parse_text_own(&format!("<p><a href=\"{long}\">x</a></p>"), |u| Some(u.to_string()));
    assert_eq!(link_bytes(&blocks), 0);
}

#[test]
fn at_the_input_cap_the_link_bytes_stay_linear() {
    // 2 MiB of alternations under an address of nearly MAX_LINK bytes.
    let html = alternations(MAX_LINK - 64, 2 * 1024 * 1024 / 9);
    let blocks = parse(&html);
    let copied = link_bytes(&blocks);
    assert!(copied <= allowed(&html), "{copied} link bytes from {} of input", html.len());
}

/// Writes the audit's payload as a seed of the HTML fuzz target (`fuzz/seeds/html/`):
/// `cargo test -p mailcraft-html --test link_spans_linear -- --ignored write_fuzz_seeds`.
#[test]
#[ignore = "writes files into the repository"]
fn write_fuzz_seeds() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/seeds/html");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("long-link-many-spans.html"), alternations(60 * 1024, 1000)).unwrap();
}
