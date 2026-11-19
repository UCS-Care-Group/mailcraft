// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The quoter's listing mode ([`pictures_in_templates`], which `image_sources` asks whether a
//! `<template>` holds a picture) follows a document exactly as its writing mode does: after every
//! token its open elements, the element it is leaving out, whether the body has begun and whether
//! raw text comes next are the writing quoter's, and it writes nothing. A lister that stopped
//! following the open elements would end a hidden cell or item somewhere else than the writer
//! does, and judge the pictures after it wrongly. The documents are generated deterministically
//! and are few, so that a failure names its document and the suite stays fast.

use super::*;

/// What the quoter holds that decides where each element ends.
type State = (Vec<&'static str>, Option<(String, usize)>, bool, bool);

fn state(q: &Quoter<'_, '_, '_, '_>) -> State {
    (q.open.clone(), q.skip.as_ref().map(|s| (s.name.clone(), s.nested)), q.body, q.raw)
}

/// Whether a `<template>` holds each picture, by a quoter that writes: the reading the lister
/// must give.
fn held_by_writing(html: &str) -> Vec<bool> {
    let mut q = Quoter::default();
    let mut held = Vec::new();
    for token in Tokenizer::new(html) {
        if let Token::Open(tag) = &token
            && tag.name == "img"
        {
            held.push(q.skip.as_ref().is_some_and(|skip| skip.name == "template"));
        }
        q.token(token);
    }
    held
}

/// Where the listing quoter first parts from the writing one over `html`, if it does.
fn parting(html: &str) -> Option<String> {
    let mut listing = Quoter { listing: true, ..Quoter::default() };
    let mut writing = Quoter::default();
    for (at, (a, b)) in Tokenizer::new(html).zip(Tokenizer::new(html)).enumerate() {
        listing.token(a);
        writing.token(b);
        if state(&listing) != state(&writing) {
            return Some(format!("token {at}: listing {:?}, writing {:?}", state(&listing), state(&writing)));
        }
        if !listing.out.is_empty() {
            return Some(format!("token {at}: the lister wrote {:?}", listing.out));
        }
    }
    let (listed, written) = (pictures_in_templates(html), held_by_writing(html));
    (listed != written).then(|| format!("held: listing {listed:?}, writing {written:?}"))
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
        let n = u64::try_from(n).unwrap();
        usize::try_from(x.wrapping_mul(0x2545_F491_4F6C_DD1D) % n).unwrap()
    }
}

/// What documents are made of: every element left out whole, hidden blocks, table parts and list
/// items whose ends are left out, raw text, comments, stray brackets and case variants.
const PARTS: &[&str] = &[
    "<template>",
    "</template>",
    "<TEMPLATE>",
    "</TeMpLaTe>",
    "<template/>",
    "<template hidden>",
    "<template x=\">\">",
    "<svg>",
    "</svg>",
    "<math>",
    "</math>",
    "<noscript>",
    "</noscript>",
    "<select>",
    "</select>",
    "<button>",
    "</button>",
    "<object>",
    "</object>",
    "<iframe>",
    "</iframe>",
    "<head>",
    "</head>",
    "<body>",
    "</body>",
    "<html>",
    "</html>",
    "<p>",
    "</p>",
    "<p hidden>",
    "<div>",
    "</div>",
    "<div style=\"display:none\">",
    "<span style=\"visibility:hidden\">",
    "</span>",
    "<div style=\"max-height:0\">",
    "<table>",
    "</table>",
    "<tr>",
    "</tr>",
    "<td>",
    "<td hidden>",
    "</td>",
    "<th>",
    "</th>",
    "<tbody>",
    "<thead>",
    "<tfoot>",
    "<caption>",
    "</caption>",
    "<ul>",
    "</ul>",
    "<ol>",
    "</ol>",
    "<li>",
    "<li hidden>",
    "</li>",
    "<dl>",
    "</dl>",
    "<dt>",
    "<dd hidden>",
    "<xmp>",
    "</xmp>",
    "<script>",
    "</script>",
    "<style>",
    "</style>",
    "<textarea>",
    "</textarea>",
    "<title>",
    "</title>",
    "<option>",
    "<optgroup>",
    "<a title='\"&'>",
    "</a>",
    "<b>",
    "</b>",
    "<h1>",
    "</h1>",
    "<blockquote>",
    "</blockquote>",
    "<br>",
    "<hr>",
    "<center>",
    "</center>",
    "x &amp; \"<\" y",
    " ",
    "<!-- <template> -->",
    "<!-- </template> -->",
    "<![CDATA[<template>]]>",
    "</",
    "<",
    "<img hidden src=\"https://pictures.example/h.png\">",
    "<img width=\"1\" height=\"1\" src=\"https://pictures.example/t.png\">",
];

/// The `n`th document: up to 48 parts, about one in five a picture of its own.
fn document(rng: &mut Rng, n: usize) -> String {
    let mut html = String::new();
    for k in 0..=rng.below(48) {
        if rng.below(5) == 0 {
            let picture = match rng.below(4) {
                0 => format!("<img src=\"https://pictures.example/{n}-{k}.png\">"),
                1 => format!("<img src=\"cid:c{n}-{k}@pictures.example\">"),
                2 => format!("<img alt=\"a\" src='missing-{n}-{k}.png'>"),
                _ => format!("<IMG SRC=https://pictures.example/{n}-{k}.png>"),
            };
            html.push_str(&picture);
        } else {
            html.push_str(PARTS[rng.below(PARTS.len())]);
        }
    }
    html
}

/// How many generated documents the comparison reads: enough to reach every part in many
/// neighbourhoods, few enough to take well under a second.
const DOCUMENTS: usize = 3_000;

#[test]
fn the_lister_follows_a_document_as_the_writer_does() {
    let shapes = [
        "<svg><template></svg><img src=\"missing.png\">",
        "<noscript><template></noscript><img src=\"missing.png\">",
        "<p hidden><template></p><img src=\"missing.png\">",
        "<table><tr><td hidden><template></tr></table><img src=\"missing.png\">",
        "<ul><li hidden><template><li><img src=\"missing.png\"></ul>",
        "<template><body><img src=\"missing.png\">",
        "<body><template><body><img src=\"a.png\"></template><img src=\"b.png\">",
        "<template><xmp></template><img src=\"a.png\"></xmp><img src=\"b.png\">",
        "<template><template><img src=\"a.png\"></template><img src=\"b.png\"></template><img src=\"c.png\">",
    ];
    for html in shapes {
        assert_eq!(parting(html), None, "{html}");
    }
    // Nesting around the quoter's depth bound, with templates and hidden blocks inside it.
    for depth in [MAX_DEPTH - 1, MAX_DEPTH, MAX_DEPTH + 1, 2 * MAX_DEPTH] {
        for inner in ["<template>", "<p hidden>", "<svg>", "<td hidden>", "<li hidden>"] {
            let html = format!("{}{inner}{}<img src=\"missing.png\">", "<div>".repeat(depth), "</div>".repeat(depth));
            assert_eq!(parting(&html), None, "{depth} deep, {inner}");
        }
    }
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for n in 0..DOCUMENTS {
        let html = document(&mut rng, n);
        assert_eq!(parting(&html), None, "document {n}: {html}");
    }
}

/// Where a hidden cell or item ends rests on the open elements: the end of its row, table or list
/// ends it, so a `<template>` after that is the document's own and holds the picture in it.
#[test]
fn the_end_of_a_row_or_a_list_ends_a_hidden_cell_or_item_before_a_template() {
    let shapes: [(&str, &[bool]); 6] = [
        ("<table><tr><td hidden>x</tr></table><template><img src=\"missing.png\"></template>", &[true]),
        ("<table><tr><th hidden>x</tr></table><template><img src=\"missing.png\"></template>", &[true]),
        ("<table><tr><td hidden>x</table><template><img src=\"missing.png\"></template>", &[true]),
        ("<ul><li hidden>x</ul><template><img src=\"missing.png\"></template>", &[true]),
        ("<ol><li hidden>x</ol><template><img src=\"missing.png\"></template><img src=\"https://pictures.example/o.png\">", &[true, false]),
        ("<ul><li hidden>x</ul><img src=\"https://pictures.example/o.png\"><template><img src=\"missing.png\"></template>", &[false, true]),
    ];
    for (html, held) in shapes {
        assert_eq!(pictures_in_templates(html), held, "{html}: the lister");
        assert_eq!(held_by_writing(html), held, "{html}: the writer's reading");
        let listed = crate::image_sources(html).unwrap();
        assert_eq!(listed.iter().map(|s| s.in_template).collect::<Vec<_>>(), held, "{html}: image_sources");
    }
}
