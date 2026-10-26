// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What the parser makes of each message of the corpus.

use super::*;
use crate::blocks::tests::check_block;
use crate::{
    Align, Block, BlockKind, Insets, Length, LineHeight, LineStyle, MAX_BLOCKS, MAX_CELLS, MAX_INPUT, MAX_TABLE_DEPTH, Marker, Rgb, Span, Table,
    has_text, images, is_embedded, parse, text_blocks, to_text,
};

/// Every block, looking inside tables, in reading order.
fn flat(blocks: &[Block]) -> Vec<&Block> {
    fn walk<'a>(blocks: &'a [Block], out: &mut Vec<&'a Block>) {
        for b in blocks {
            out.push(b);
            if let BlockKind::Table(t) = &b.kind {
                for cell in t.rows.iter().flat_map(|r| &r.cells) {
                    walk(&cell.blocks, out);
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(blocks, &mut out);
    out
}

fn tables(blocks: &[Block]) -> Vec<&Table> {
    flat(blocks)
        .into_iter()
        .filter_map(|b| match &b.kind {
            BlockKind::Table(t) => Some(t),
            _ => None,
        })
        .collect()
}

/// All the text, a block per line.
fn text_of(blocks: &[Block]) -> String {
    flat(blocks).iter().map(|b| b.text()).collect::<Vec<_>>().join("\n")
}

/// The first block whose text starts with `start`.
fn block<'a>(blocks: &'a [Block], start: &str) -> &'a Block {
    flat(blocks).into_iter().find(|b| b.text().starts_with(start)).unwrap_or_else(|| panic!("no block starts with {start:?}\n{}", show(blocks)))
}

/// The span that holds `text`.
fn span<'a>(blocks: &'a [Block], text: &str) -> &'a Span {
    flat(blocks)
        .into_iter()
        .flat_map(|b| &b.spans)
        .find(|s| s.text.contains(text))
        .unwrap_or_else(|| panic!("no span holds {text:?}\n{}", show(blocks)))
}

/// The table one of whose cells holds a block starting with `start`, directly.
fn table_with<'a>(blocks: &'a [Block], start: &str) -> &'a Table {
    tables(blocks)
        .into_iter()
        .find(|t| t.rows.iter().flat_map(|r| &r.cells).any(|c| c.blocks.iter().any(|b| b.text().starts_with(start))))
        .unwrap_or_else(|| panic!("no table holds {start:?}\n{}", show(blocks)))
}

fn depth(blocks: &[Block]) -> usize {
    blocks
        .iter()
        .map(|b| match &b.kind {
            BlockKind::Table(t) => 1 + t.rows.iter().flat_map(|r| &r.cells).map(|c| depth(&c.blocks)).max().unwrap_or(0),
            _ => 0,
        })
        .max()
        .unwrap_or(0)
}

fn cells(blocks: &[Block]) -> usize {
    tables(blocks).iter().map(|t| t.rows.iter().map(|r| r.cells.len()).sum::<usize>()).sum()
}

/// Every address a block could lead to or show: links and pictures.
fn addresses(blocks: &[Block]) -> Vec<String> {
    let mut out = Vec::new();
    for b in flat(blocks) {
        if let BlockKind::Image { src, link, .. } = &b.kind {
            out.push(src.clone());
            out.extend(link.clone());
        }
        out.extend(b.spans.iter().filter_map(|s| s.link.clone()));
    }
    out.sort();
    out.dedup();
    out
}

/// The block tree, a line per block, for reading when a test fails.
fn outline(blocks: &[Block], indent: usize, out: &mut String) {
    for b in blocks {
        let pad = "  ".repeat(indent);
        let head = match &b.kind {
            BlockKind::Table(t) => format!(
                "{} {}x{}{}{}{}{}",
                if t.layout { "layout" } else { "data" },
                t.rows.len(),
                t.columns,
                if t.border { " border" } else { "" },
                if t.outline { " outline" } else { "" },
                t.width.map_or(String::new(), |w| format!(" w={w:?}")),
                t.background.map_or(String::new(), |c| format!(" bg={c:?}")),
            ),
            other => format!("{other:?}"),
        };
        let text: String = b.text().chars().take(60).collect();
        out.push_str(&format!("{pad}{head} q{} s{} {:?} | {text}\n", b.quote, b.space, b.align));
        if let BlockKind::Table(t) = &b.kind {
            for (r, row) in t.rows.iter().enumerate() {
                for (c, cell) in row.cells.iter().enumerate() {
                    out.push_str(&format!(
                        "{pad}  [{r},{c}] span {}x{} bg={:?} w={:?} th={} border={:?}\n",
                        cell.colspan, cell.rowspan, cell.background, cell.width, cell.header, cell.border
                    ));
                    outline(&cell.blocks, indent + 2, out);
                }
            }
        }
    }
}

fn show(blocks: &[Block]) -> String {
    let mut out = String::new();
    outline(blocks, 0, &mut out);
    out
}

/// Every picture, in reading order: its address, the size it asks for, and where it leads.
fn pictures_of(blocks: &[Block]) -> Vec<(&str, Option<u16>, Option<u16>, Option<&str>)> {
    images(blocks)
        .into_iter()
        .filter_map(|b| match &b.kind {
            BlockKind::Image { src, width, height, link, .. } => Some((src.as_str(), *width, *height, link.as_deref())),
            _ => None,
        })
        .collect()
}

#[test]
#[ignore = "prints the block tree of every corpus message, for reading"]
fn print_outlines() {
    for (name, html) in messages() {
        println!("==== {name}\n{}", show(&parse(html)));
    }
    println!("==== hostile colours\n{}", show(&parse(HOSTILE_COLOURS)));
    println!("==== hostile remote\n{}", show(&parse(HOSTILE_REMOTE)));
    println!("==== plain thread\n{}", show(&text_blocks(PLAIN_THREAD)));
}

#[test]
fn every_message_parses_into_sane_blocks() {
    for (name, html) in messages() {
        let blocks = parse(html);
        assert!(has_text(&blocks), "{name}");
        assert!(depth(&blocks) <= 4, "{name}: tables {} deep\n{}", depth(&blocks), show(&blocks));
        assert_eq!(blocks.first().map(|b| b.space), Some(0), "{name}");
        for b in &blocks {
            check_block(b, name);
        }
        // What a style sheet, a title or a comment says is never text.
        let text = text_of(&blocks);
        for never in ["font-family", "mso-", "@media", "@import", "<!--", "endif", "DOCTYPE", "{", "}"] {
            assert!(!text.contains(never), "{name}: {never:?} in the text");
        }
        // Plain text comes out of every message too.
        assert!(to_text(html).len() > 200, "{name}");
    }
}

#[test]
fn outlook_reply() {
    let blocks = parse(OUTLOOK_REPLY);
    // Lines follow each other directly; an empty paragraph is a blank line above the next.
    assert_eq!(blocks.first().map(|b| (b.text(), b.space)), Some(("Hi Owen,".to_string(), 0)));
    assert_eq!(block(&blocks, "Thanks for turning").space, 1);
    assert_eq!((block(&blocks, "Kind regards").space, block(&blocks, "Marta").space), (1, 0));
    assert_eq!((block(&blocks, "Marta Lindqvist").space, block(&blocks, "Pricing Lead").space), (1, 0));
    assert!(block(&blocks, "Pricing Lead").text().contains("Tidewater Freight\nDirect +45"));
    // Its list paragraphs are list items, with real markers instead of the written ones.
    let item = |start: &str| (block(&blocks, start).kind.clone(), block(&blocks, start).space);
    assert_eq!(item("The fuel surcharge"), (BlockKind::ListItem { marker: Marker::Bullet, depth: 0 }, 1));
    assert_eq!(item("Saturday only"), (BlockKind::ListItem { marker: Marker::Bullet, depth: 1 }, 0));
    assert_eq!(item("Who signs"), (BlockKind::ListItem { marker: Marker::Bullet, depth: 0 }, 0));
    assert_eq!(item("Send the two settled"), (BlockKind::ListItem { marker: Marker::Number(1), depth: 0 }, 0));
    assert_eq!(item("Hold Halden"), (BlockKind::ListItem { marker: Marker::Number(2), depth: 0 }, 0));
    let text = text_of(&blocks);
    assert!(!text.contains('·') && !text.contains("1.\u{a0}") && !text.contains("o\u{a0}"), "a written marker is left in the text");
    // Inline formatting from the whitelist.
    assert!(span(&blocks, "4.5%").bold);
    assert_eq!(span(&blocks, "two-hour").color, Some(Rgb(255, 0, 0)));
    assert_eq!(span(&blocks, "The Halden figure is provisional").background, Some(Rgb(255, 255, 0)));
    assert_eq!(span(&blocks, "pricing share").link.as_deref(), Some("https://files.tidewater-freight.example/pricing/q4-lanes"));
    let name = span(&blocks, "Marta Lindqvist");
    assert_eq!((name.bold, name.color, name.size), (true, Some(Rgb(0x1f, 0x38, 0x64)), Some(13)));
    let small = span(&blocks, "This message is for the person");
    assert_eq!((small.color, small.size), (Some(Rgb(128, 128, 128)), Some(11)));
    // The Word table is a data table with a line round every cell.
    let table = table_with(&blocks, "Port Sorrel to Halden");
    assert!(!table.layout && table.border);
    assert_eq!((table.rows.len(), table.columns), (4, 3));
    let head: Vec<_> = table.rows.first().map(|r| r.cells.iter().map(|c| (c.background, c.width)).collect()).unwrap_or_default();
    assert_eq!(head.first(), Some(&(Some(Rgb(0xd9, 0xe2, 0xf3)), Some(Length::Px(180)))));
    assert_eq!(block(&blocks, "1,840.00").align, Align::Right);
    assert_eq!(span(&blocks, "1,020.00").color, Some(Rgb(0xc0, 0, 0)));
    // The logo is a picture of the message's own, with the size it asks for.
    let pictures = images(&blocks);
    assert_eq!(pictures.len(), 1);
    assert_eq!(
        pictures.first().map(|b| (&b.kind, b.text())),
        Some((
            &BlockKind::Image { src: "cid:image001.png@01DC3A1B.5F0C2E40".into(), width: Some(168), height: Some(48), link: None, data: None },
            "Tidewater Freight".to_string()
        ))
    );
    // Each quoted message starts under a rule, with its "From:" block right below.
    let rules: Vec<usize> = blocks.iter().enumerate().filter(|(_, b)| b.kind == BlockKind::Rule).map(|(i, _)| i).collect();
    assert_eq!(rules.len(), 2);
    for i in rules {
        let after = blocks.get(i + 1).unwrap_or_else(|| panic!("nothing after the rule"));
        assert!(after.text().starts_with("From: ") && after.space == 0, "{after:?}");
        assert!(after.text().contains("\nSent: ") && after.text().contains("\nSubject: "));
    }
    assert_eq!(span(&blocks, "Revised schedule attached").color, Some(Rgb(0x1f, 0x49, 0x7d)));
    // As plain text.
    let plain = to_text(OUTLOOK_REPLY);
    for line in [
        "• The fuel surcharge cap",
        "  • Saturday only, as last year",
        "1. Send the two settled lanes",
        "Port Sorrel to Halden | 1,840.00 | 1,915.00",
        "[Tidewater Freight]",
        "----------",
    ] {
        assert!(plain.contains(line), "{line:?} not in:\n{plain}");
    }
}

#[test]
fn gmail_reply() {
    let blocks = parse(GMAIL_REPLY);
    assert!(tables(&blocks).is_empty());
    let quote = |start: &str| block(&blocks, start).quote;
    assert_eq!((quote("Hi Priya"), quote("On Tue, Oct 6")), (0, 0));
    assert_eq!((quote("Morning Kofi"), quote("Where we are"), quote("On Mon, Oct 5, 2026 at 5:40")), (1, 1, 1));
    assert_eq!((quote("Priya, here are my notes"), quote("On Mon, Oct 5, 2026 at 2:02")), (2, 2));
    assert_eq!(quote("How did the call go?"), 3);
    assert_eq!(blocks.iter().map(|b| b.quote).max(), Some(3));
    // A `<div>` per line: lines are tight, `<div><br></div>` is a blank line.
    assert_eq!((block(&blocks, "Yes to both").space, block(&blocks, "See you Thursday").space, block(&blocks, "Kofi").space), (1, 1, 0));
    // Its quote has no margin of its own: the quoted text starts right under "… wrote:".
    assert_eq!(block(&blocks, "Morning Kofi").space, 0);
    assert_eq!(block(&blocks, "Where we are").kind, BlockKind::ListItem { marker: Marker::Number(1), depth: 0 });
    assert_eq!(block(&blocks, "The demo should come").kind, BlockKind::ListItem { marker: Marker::Bullet, depth: 0 });
    assert!(span(&blocks, "before").italic && span(&blocks, "Agenda (draft)").bold);
    assert_eq!(span(&blocks, "the wrong week").background, Some(Rgb(255, 242, 204)));
    assert_eq!(span(&blocks, "written plan").link.as_deref(), Some("https://docs.juniperlabs.example/migration/plan"));
    let small = span(&blocks, "Platform team");
    assert_eq!((small.color, small.size), (Some(Rgb(0x66, 0x66, 0x66)), Some(10)));
    assert!(to_text(GMAIL_REPLY).contains(">>> How did the call go?"));
}

#[test]
fn newsletter() {
    let blocks = parse(NEWSLETTER);
    let text = text_of(&blocks);
    // What is hidden stays hidden: the preheader, the preview-only text, the mobile menu.
    for hidden in ["Wool layers are back", "PREVIEW-ONLY", "Tap here for the mobile menu", "Autumn on the ridge: new arrivals", "\u{200c}"] {
        assert!(!text.contains(hidden), "{hidden:?} is shown");
    }
    // The page colour is one box, and in it the white column, 600 wide and centred.
    let [page] = blocks.as_slice() else { panic!("{}", show(&blocks)) };
    let BlockKind::Table(page) = &page.kind else { panic!("{}", show(&blocks)) };
    let cell = page.rows.first().and_then(|r| r.cells.first()).unwrap_or_else(|| panic!("no page cell"));
    assert_eq!((page.rows.len(), cell.background), (1, Some(Rgb(0xee, 0xf1, 0xec))));
    let [column] = cell.blocks.as_slice() else { panic!("{}", show(&blocks)) };
    let BlockKind::Table(table) = &column.kind else { panic!("{}", show(&blocks)) };
    assert_eq!((table.width, table.background, column.align, table.layout), (Some(Length::Px(600)), Some(Rgb(255, 255, 255)), Align::Center, true));
    assert!(tables(&blocks).iter().all(|t| t.layout), "a layout table was taken for data\n{}", show(&blocks));
    // Every picture is remote, so every picture is a placeholder; the trackers are not even that.
    let pictures = images(&blocks);
    assert_eq!(pictures.len(), 8, "{}", show(&blocks));
    for picture in &pictures {
        let BlockKind::Image { src, .. } = &picture.kind else { continue };
        assert!(!is_embedded(src) && src.starts_with("https://img.fernhollow.example/"), "{src}");
    }
    assert!(!addresses(&blocks).iter().any(|a| a.contains("/o/v1/") || a.contains("pixel.") || a.contains("fonts.") || a.contains("bg/leaves")));
    let hero = block(&blocks, "Two walkers on a ridge");
    assert_eq!(
        hero.kind,
        BlockKind::Image {
            src: "https://img.fernhollow.example/2026/10/hero-ridge-morning.jpg".into(),
            width: Some(600),
            height: Some(280),
            link: Some("https://click.fernhollow.example/t/v1/9f3a7c?u=88213&l=hero".into()),
            data: None,
        }
    );
    // Text keeps its colours, sizes and alignment.
    let title = block(&blocks, "Autumn on the ridge");
    assert_eq!((&title.kind, title.align), (&BlockKind::Heading(1), Align::Left));
    assert_eq!(title.spans.first().map(|s| (s.bold, s.color, s.size)), Some((false, Some(Rgb(0x22, 0x33, 0x1f)), Some(30))));
    assert_eq!(block(&blocks, "Trouble reading this?").align, Align::Center);
    let body = span(&blocks, "The first frost came");
    assert_eq!((body.color, body.size), (Some(Rgb(0x3d, 0x46, 0x3b)), Some(16)));
    assert!(span(&blocks, "forty pairs of boots").bold);
    assert_eq!((block(&blocks, "The first frost came").space, block(&blocks, "Members get first pick").space), (0, 1));
    let was = span(&blocks, "89.00");
    assert_eq!((was.strike, was.color), (true, Some(Rgb(0x9a, 0xa3, 0x97))));
    assert_eq!((span(&blocks, "72.00 for members").bold, span(&blocks, "72.00 for members").color), (true, Some(Rgb(0xc8, 0x55, 0x2b))));
    // The button is a cell with a background, white text that leads somewhere.
    let button = table_with(&blocks, "See the members' weekend");
    let cell = button.rows.first().and_then(|r| r.cells.first()).unwrap_or_else(|| panic!("no button cell"));
    assert_eq!((button.rows.len(), button.columns, cell.background), (1, 1, Some(Rgb(0xc8, 0x55, 0x2b))));
    let label = span(&blocks, "See the members' weekend");
    assert_eq!(
        (label.color, label.bold, label.link.as_deref()),
        (Some(Rgb(255, 255, 255)), true, Some("https://click.fernhollow.example/t/v1/9f3a7c?u=88213&l=cta-weekend"))
    );
    // Two products side by side, half the width each.
    let products = table_with(&blocks, "Merino crew");
    let widths: Vec<_> = products.rows.iter().flat_map(|r| &r.cells).map(|c| c.width).collect();
    assert_eq!(widths, [Some(Length::Percent(50)), Some(Length::Percent(50))]);
    // The line between the sections is a cell with a line above it and nothing in it.
    assert!(tables(&blocks).iter().flat_map(|t| &t.rows).flat_map(|r| &r.cells).any(|c| c.border.top && c.blocks.is_empty()), "{}", show(&blocks));
    // The three small pictures of the last row stand in one line.
    assert_eq!((block(&blocks, "Photos").space, block(&blocks, "Films").space, block(&blocks, "Journal").space), (0, 0, 0));
    let footer = span(&blocks, "You are getting this because");
    assert_eq!((footer.color, footer.size), (Some(Rgb(0x8a, 0x93, 0x86)), Some(11)));
    assert_eq!(span(&blocks, "unsubscribe").link.as_deref(), Some("https://click.fernhollow.example/u/9f3a7c?u=88213"));
}

#[test]
fn built_newsletter() {
    let blocks = parse(BUILT_NEWSLETTER);
    let text = text_of(&blocks);
    // Everything in it sits in cells with a font size of nothing; all of it is there.
    for shown in [
        "Lamplight · No. 41 · Read it on the web",
        "The evenings are drawing in",
        "Forty hours of light in a jar, poured on Tuesday.",
        "The clocks went back on Sunday",
        "If you have never seen a candle dipped",
        "Amber Glow\nForty hours, orange peel and clove\n24.00",
        "Winter tapers\nSix to a bundle, natural or oxblood\n18.50",
        "You are reading Lamplight because you asked for it",
    ] {
        assert!(text.contains(shown), "{shown:?} is lost\n{}", show(&blocks));
    }
    // What is hidden from everybody, and what is written for Outlook alone, is not.
    for never in ["Beeswax tapers are back", "BUTTON-DRAWN-FOR-OUTLOOK", "FOR-OUTLOOK-ONLY", "\u{34f}", "\u{200c}", "font-family", "endif"] {
        assert!(!text.contains(never), "{never:?} is shown");
    }
    assert!(tables(&blocks).iter().all(|t| t.layout), "{}", show(&blocks));
    // Its pictures, each once, in reading order: the address as it is to be asked for, the size
    // from the attributes or else from the style, and where the picture leads.
    let go = |what: &str| format!("https://go.tallowwick.example/c/7Qk2/{what}");
    let (home, hero) = (go("home?m=40117&r=lamplight-41"), go("hero?m=40117&r=lamplight-41&utm_medium=email"));
    let (amber, tapers, open_day) = (go("p-1181?m=40117"), go("p-1204?m=40117"), go("open-day?m=40117"));
    let (photos, letters) = (go("s-photos?m=40117"), go("s-letters?m=40117"));
    assert_eq!(
        pictures_of(&blocks),
        [
            // No scheme: `https`. No size but in its style.
            ("https://img.tallowwick.example/brand/wordmark-ink.png", Some(180), Some(40), Some(home.as_str())),
            // The scheme in capitals, the query with its `&amp;` read; `height="auto"` is no height.
            ("HTTPS://img.tallowwick.example/2026/11/hero-dusk-table.jpg?w=1200&q=72&sig=9c1e_f0a7", Some(600), None, Some(hero.as_str())),
            // A space in the address stays the address's own; a line broken in it is mended.
            ("https://img.tallowwick.example/2026/11/products/amber glow jar.jpg", Some(262), Some(262), Some(amber.as_str())),
            ("https://img.tallowwick.example/2026/11/products/winter-tapers-six.jpg", Some(262), Some(262), Some(tapers.as_str())),
            // One picture in slices, each in a cell with no font size at all.
            ("https://img.tallowwick.example/2026/11/open-day/slice-1.png", Some(600), Some(96), Some(open_day.as_str())),
            ("https://img.tallowwick.example/2026/11/open-day/slice-2.png", Some(600), Some(120), Some(open_day.as_str())),
            // The one for every program but Outlook.
            ("https://img.tallowwick.example/people/signature-maud@2x.png", Some(132), Some(48), None),
            ("https://img.tallowwick.example/icons/round-photos.png", Some(28), Some(28), Some(photos.as_str())),
            ("https://img.tallowwick.example/icons/round-letters.png", Some(28), Some(28), Some(letters.as_str())),
        ],
        "{}",
        show(&blocks)
    );
    assert_eq!(block(&blocks, "Tallow & Wick Candle Works").kind, images(&blocks).first().map(|b| b.kind.clone()).unwrap_or(BlockKind::Rule));
    // The two small pictures of the last row stand in one line.
    assert_eq!((block(&blocks, "Photographs").space, block(&blocks, "Letters").space), (0, 0));
    // Nothing else it names is anywhere in the result: not the background picture, given as an
    // attribute, in the style, in a style sheet and in VML, nor `srcset`, nor the pixels.
    let kept = format!("{blocks:?}").to_ascii_lowercase();
    for gone in ["/bg/", "linen", "@3x", "for-outlook", "/o/7qk2", "metrics.", "seen.gif", "v:rect", "roundrect", "mso"] {
        assert!(!kept.contains(gone), "{gone} survived");
    }
    // The cell that has the background picture keeps its colour, and the light text on it.
    let over = table_with(&blocks, "The evenings are drawing in");
    assert_eq!(over.rows.first().and_then(|r| r.cells.first()).and_then(|c| c.background), Some(Rgb(0x3b, 0x2f, 0x2a)));
    let title = span(&blocks, "The evenings are drawing in");
    assert_eq!(
        (title.color, title.size, block(&blocks, "The evenings are drawing in").align),
        (Some(Rgb(0xfb, 0xf6, 0xec)), Some(30), Align::Center)
    );
    // The button everybody but Outlook gets: its text, in its colours, leading where it leads.
    let button = span(&blocks, "See the winter tapers");
    assert_eq!(
        (button.color, button.background, button.bold, button.link.as_deref()),
        (Some(Rgb(255, 255, 255)), Some(Rgb(0xb5, 0x56, 0x2b)), true, Some("https://go.tallowwick.example/c/7Qk2/cta?m=40117&r=lamplight-41"))
    );
    // Text keeps the sizes and colours its own blocks give it.
    let body = span(&blocks, "The clocks went back on Sunday");
    assert_eq!((body.color, body.size), (Some(Rgb(0x3d, 0x38, 0x33)), Some(16)));
    assert!(span(&blocks, "winter tapers").bold);
    assert_eq!(span(&blocks, "24.00").color, Some(Rgb(0xb5, 0x56, 0x2b)));
    assert_eq!(span(&blocks, "Stop these letters").link.as_deref(), Some("https://go.tallowwick.example/u/7Qk2?m=40117"));
    let plain = to_text(BUILT_NEWSLETTER);
    assert!(plain.contains("The evenings are drawing in") && plain.contains("[A table by a window at dusk, three candles lit]"), "{plain}");
}

#[test]
fn receipt() {
    let blocks = parse(RECEIPT);
    assert_eq!(block(&blocks, "Thanks, Noor").kind, BlockKind::Heading(1));
    // The facts of the order: a grid of short texts, so data, though nothing says so.
    let facts = table_with(&blocks, "Paid with");
    assert!(!facts.layout && !facts.border);
    assert_eq!((facts.rows.len(), facts.columns), (4, 2));
    assert_eq!(span(&blocks, "Paid with").color, Some(Rgb(0x6b, 0x6b, 0x6b)));
    // The items: header cells, a shaded header row, lines under the rows, amounts to the right.
    let items = table_with(&blocks, "Brass fountain pen");
    assert!(!items.layout);
    assert_eq!((items.rows.len(), items.columns, items.width), (8, 4, Some(Length::Percent(100))));
    let head = items.rows.first().unwrap_or_else(|| panic!("no header row"));
    assert!(head.cells.iter().all(|c| c.header && c.background == Some(Rgb(0xf6, 0xf2, 0xe7)) && c.border.bottom && !c.border.top));
    let aligns: Vec<Align> = head.cells.iter().filter_map(|c| c.blocks.first()).map(|b| b.align).collect();
    assert_eq!(aligns, [Align::Left, Align::Right, Align::Right, Align::Right]);
    assert!(span(&blocks, "Qty").bold);
    let row = items.rows.get(1).unwrap_or_else(|| panic!("no first item"));
    assert!(row.cells.iter().all(|c| c.border.bottom && !c.header && c.background.is_none()));
    assert!(block(&blocks, "Dot-grid notebook").text().ends_with("\nColour: ink blue"));
    assert_eq!(span(&blocks, "Colour: ink blue").size, Some(12));
    assert_eq!(span(&blocks, "Back in stock").color, Some(Rgb(0xb3, 0x26, 0x1e)));
    // The totals span three columns.
    let total = items.rows.last().unwrap_or_else(|| panic!("no total row"));
    assert_eq!(total.cells.iter().map(|c| c.colspan).collect::<Vec<_>>(), [3, 1]);
    assert!(total.cells.iter().all(|c| c.border.top));
    assert!(span(&blocks, "Total paid").bold && span(&blocks, "79.83").bold);
    assert_eq!(span(&blocks, "\u{2212}8.32").color, Some(Rgb(0x2e, 0x7d, 0x32)));
    assert_eq!(block(&blocks, "79.83").align, Align::Right);
    // The note is a tinted box with a line down its left.
    let note = table_with(&blocks, "Something not right?");
    assert_eq!(note.rows.first().and_then(|r| r.cells.first()).and_then(|c| c.background), Some(Rgb(0xff, 0xf8, 0xe1)));
    assert!(flat(&blocks).iter().any(|b| matches!(&b.kind, BlockKind::Table(t) if std::ptr::eq(t, note)) && b.quote == 1));
    let plain = to_text(RECEIPT);
    assert!(plain.contains("Item | Qty | Price | Total\nDot-grid notebook, A5, cloth cover Colour: ink blue | 2 | 14.50 | 29.00"), "{plain}");
    assert!(plain.contains("Total paid | 79.83"));
}

#[test]
fn screenshots() {
    let blocks = parse(SCREENSHOTS);
    let pictures = images(&blocks);
    let srcs: Vec<&str> = pictures
        .iter()
        .filter_map(|b| match &b.kind {
            BlockKind::Image { src, .. } => src.strip_prefix("cid:"),
            _ => None,
        })
        .collect();
    assert_eq!(
        srcs,
        [
            "f3b1c2d4-export-dialog@quillon.example",
            "a77e0919-expected@quillon.example",
            "tick-16@quillon.example",
            "tick-16@quillon.example",
            "cross-16@quillon.example",
            MISSING_PICTURE,
            "damaged-capture@quillon.example",
            "quillon-mark@quillon.example",
        ]
    );
    // Every picture here is the message's own, and every one the corpus lists is referred to.
    assert_eq!(pictures.len(), srcs.len());
    for (id, ..) in PICTURES.iter().skip(1) {
        assert!(srcs.contains(id), "{id}");
    }
    assert!(!PICTURES.iter().any(|(id, ..)| *id == MISSING_PICTURE));
    let size = |alt: &str| match &block(&blocks, alt).kind {
        BlockKind::Image { width, height, .. } => (*width, *height),
        other => panic!("{other:?}"),
    };
    assert_eq!(size("The export dialog as it should look"), (None, None));
    assert_eq!(size("failed"), (Some(16), Some(16)));
    assert_eq!(
        pictures.first().map(|b| (&b.kind, b.space)).map(|(k, s)| (matches!(k, BlockKind::Image { width: Some(520), height: Some(300), .. }), s)),
        Some((true, 1))
    );
    // The three small pictures follow "Status so far:" in its line.
    let status: Vec<u8> = flat(&blocks).iter().skip_while(|b| !b.text().starts_with("Status so far")).skip(1).take(3).map(|b| b.space).collect();
    assert_eq!(status, [0, 0, 0]);
    // Text keeps its colour and size (12pt is 16 pixels).
    let text = span(&blocks, "The export dialog is still wrong");
    assert_eq!((text.color, text.size), (Some(Rgb(0, 0, 0)), Some(16)));
    assert_eq!(span(&blocks, "Delimiter").color, Some(Rgb(196, 43, 28)));
    // The signature: a logo beside three lines, with a line between them.
    let signature = table_with(&blocks, "Elias Brandt");
    assert!(signature.layout);
    let cells: Vec<_> = signature.rows.iter().flat_map(|r| &r.cells).collect();
    assert_eq!(cells.len(), 2);
    assert!(cells.first().is_some_and(|c| matches!(c.blocks.first().map(|b| &b.kind), Some(BlockKind::Image { width: Some(56), .. }))));
    assert!(cells.get(1).is_some_and(|c| c.border.left && !c.border.top && c.blocks.len() == 3));
    assert_eq!(span(&blocks, "+1 555 0163").link.as_deref(), Some("tel:+15550163"));
}

#[test]
fn calendar_notice() {
    let blocks = parse(CALENDAR);
    let text = text_of(&blocks);
    assert!(!text.contains("Invitation: Design review"), "the preheader is shown");
    // A framed card, then the small print.
    let [card, small] = blocks.as_slice() else { panic!("{}", show(&blocks)) };
    let BlockKind::Table(card) = &card.kind else { panic!("{}", show(&blocks)) };
    assert!(card.outline && card.layout && card.width == Some(Length::Percent(100)));
    assert_eq!((small.spans.first().and_then(|s| s.size), small.space), (Some(11), 1));
    assert_eq!(block(&blocks, "Design review: onboarding flow v3").kind, BlockKind::Heading(2));
    assert!(!span(&blocks, "Design review: onboarding flow v3").bold);
    // When, where, who: a grid of labels and values.
    let facts = table_with(&blocks, "Where");
    assert!(!facts.layout && !facts.border);
    assert_eq!((facts.rows.len(), facts.columns), (4, 2));
    assert!(span(&blocks, "When").bold);
    assert_eq!(span(&blocks, "meet.lumen-works.example/dsn-rvw-okx").link.as_deref(), Some("https://meet.lumen-works.example/dsn-rvw-okx"));
    // The questions: a numbered list with a bullet under its third item.
    let marker = |start: &str| block(&blocks, start).kind.clone();
    assert_eq!(marker("Team name before"), BlockKind::ListItem { marker: Marker::Number(1), depth: 0 });
    assert_eq!(marker("Whether the shortcuts page"), BlockKind::ListItem { marker: Marker::Number(3), depth: 0 });
    assert_eq!(marker("and if so, where"), BlockKind::ListItem { marker: Marker::Bullet, depth: 1 });
    // The answers: a filled button and two outlined ones in a row, the gaps between them gone.
    let answers = table_with(&blocks, "Maybe");
    assert!(answers.layout);
    let cells: Vec<_> = answers.rows.iter().flat_map(|r| &r.cells).collect();
    assert_eq!(
        cells.iter().map(|c| (c.background, c.border.any())).collect::<Vec<_>>(),
        [(None, false), (Some(Rgb(0x1a, 0x73, 0xe8)), false), (None, true), (None, true)]
    );
    let yes = span(&blocks, "Yes");
    assert_eq!(
        (yes.color, yes.bold, yes.link.as_deref()),
        (Some(Rgb(255, 255, 255)), true, Some("https://calendar.lumen-works.example/rsvp?e=4f8a&r=yes"))
    );
    assert_eq!(span(&blocks, "Maybe").color, Some(Rgb(0x1a, 0x73, 0xe8)));
}

#[test]
fn plain_text_thread() {
    let blocks = text_blocks(PLAIN_THREAD);
    assert!(blocks.iter().all(|b| b.kind == BlockKind::Paragraph));
    let quote = |start: &str| block(&blocks, start).quote;
    assert_eq!((quote("Hi all"), quote("Thanks Jorge"), quote("rc2 is up"), quote("On Fri, 9 Oct")), (0, 1, 2, 3));
    assert_eq!(blocks.iter().map(|b| b.quote).max(), Some(3));
    // The long first paragraph is one block (it wraps when shown), with its own line breaks kept.
    assert!(block(&blocks, "Short answer").text().len() > 250 && !block(&blocks, "Short answer").text().contains('\n'));
    assert!(block(&blocks, "  - importer no longer").text().contains("\n  - the --dry-run flag"));
    // Addresses can be followed.
    let links = addresses(&blocks);
    for link in [
        "https://builds.orchard-tools.example/nightly/2026-10-12/orchard-4.8.1-rc2.tar.gz",
        "https://www.orchard-tools.example/lists/dev",
        "mailto:release@orchard-tools.example",
        "mailto:hanna@orchard-tools.example",
    ] {
        assert!(links.iter().any(|l| l == link), "{link} not in {links:?}");
    }
    // A reply right under the lines it answers; the signature after its marker.
    assert_eq!((block(&blocks, "1. Fixed.").space, block(&blocks, "Thanks Jorge").space), (1, 0));
    let last: Vec<String> = blocks.iter().rev().take(2).map(Block::text).collect();
    assert!(last.first().is_some_and(|t| t.starts_with("--\nJorge Salas\nRelease wrangler")), "{last:?}");
    assert_eq!(last.get(1).map(String::as_str), Some("Jorge"));
}

#[test]
fn nothing_remote_or_active_survives() {
    let blocks = parse(HOSTILE_REMOTE);
    let text = text_of(&blocks);
    for shown in ["VISIBLE-ONE", "VISIBLE-TWO", "VISIBLE-THREE", "VISIBLE-FOUR", "VISIBLE-FIVE"] {
        assert!(text.contains(shown), "{shown} is lost\n{}", show(&blocks));
    }
    for never in ["HIDDEN-ONE", "iframe fallback", "fetch(", "document.write", "@import", "evil.example/landing"] {
        assert!(!text.contains(never), "{never:?} is shown");
    }
    // The only addresses left are the pictures' (which are never loaded) and two links that may be followed.
    let links: Vec<String> = flat(&blocks).iter().flat_map(|b| &b.spans).filter_map(|s| s.link.clone()).collect();
    assert_eq!(links, ["https://evil.example/click", "mailto:someone@elsewhere.example"]);
    let pictures: Vec<&str> = images(&blocks)
        .iter()
        .filter_map(|b| match &b.kind {
            BlockKind::Image { src, .. } => Some(src.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        pictures,
        [
            "https://evil.example/plain.gif",
            "http://evil.example/insecure.gif",
            // Written without its scheme: read as `https`, a stand-in like the others.
            "https://evil.example/schemeless.gif",
            // Written into the message (#533): shown as an attached one is, its content not kept here.
            "data:image/gif",
            "cid:not-attached@evil.example",
            "https://evil.example/lazy.gif",
            "https://evil.example/picture-fallback.jpg",
            "https://evil.example/noscript.gif",
            "https://evil.example/mathml.gif",
        ]
    );
    // Nothing else of the forty-odd addresses in the message is anywhere in the result.
    let kept = format!("{blocks:?}");
    for gone in [
        "track.css",
        "preload.gif",
        "import.css",
        "font.woff2",
        "body-bg.gif",
        "after.gif",
        "payload.js",
        "beacon",
        "written.gif",
        "attr-bg.gif",
        "onload.gif",
        "relative.gif",
        "set-1x.gif",
        "set-2x.gif",
        "base64",
        "secret.png",
        "smb.png",
        "pixel.gif",
        "picture-source.webp",
        "video.mp4",
        "poster.jpg",
        "audio.mp3",
        "object.swf",
        "embed.swf",
        "frame.html",
        "frame2.html",
        "svg-image.png",
        "svg-xlink.png",
        "sprite.svg",
        "input.gif",
        "collect",
        "table-bg.gif",
        "cell-bg.gif",
        "style-bg.gif",
        "shorthand-bg.gif",
        "div-bg.gif",
        "list.gif",
        "cursor.cur",
        "border.png",
        "javascript",
        "vbscript",
        "passwd",
        "ms-msdt",
        "/ping",
        "area",
        "sound.wav",
        "template.gif",
        "comment.gif",
        "cdata.gif",
        "/base/",
    ] {
        assert!(!kept.to_ascii_lowercase().contains(gone), "{gone} survived");
    }
    // The one background colour that came with a picture is kept; its picture is not.
    assert_eq!(
        table_with(&blocks, "VISIBLE-THREE").rows.first().and_then(|r| r.cells.first()).and_then(|c| c.background),
        Some(Rgb(0xff, 0xee, 0xee))
    );
}

#[test]
fn colours_cannot_remove_text() {
    let blocks = parse(HOSTILE_COLOURS);
    let text = text_of(&blocks);
    for shown in [
        "WHITE-ON-NOTHING",
        "BLACK-ON-NOTHING",
        "SURFACE-DARK",
        "SURFACE-LIGHT",
        "WHITE-ON-WHITE",
        "BLACK-ON-BLACK",
        "YELLOW",
        "CYAN",
        "TRANSPARENT",
        "KEYWORD",
        "GREY-ON-GREY",
        "DARK-BOX-PLAIN",
        "DARK-BOX-DARK",
        "DARK-BOX-LINK",
        "LIGHT-BOX-PLAIN",
        "LIGHT-BOX-LIGHT",
        "LIGHT-BOX-LINK",
        "NAVY-HEAD",
        "NAVY-ON-NAVY",
        "YELLOW-ON-YELLOW",
        "GREEN-ON-RED",
        "SMALL",
        "HUGE",
        "HEADING",
        "LIST",
        "QUOTE",
        "PRE",
    ] {
        assert!(text.contains(shown), "{shown} is lost");
    }
    // Text too small to be meant to be read is the one thing not shown.
    assert!(!text.contains("TINY"));
    // The colours are passed on as asked for: whoever paints them sees to the contrast.
    assert_eq!(
        (span(&blocks, "WHITE-ON-WHITE").color, span(&blocks, "WHITE-ON-WHITE").background),
        (Some(Rgb(255, 255, 255)), Some(Rgb(255, 255, 255)))
    );
    assert_eq!(span(&blocks, "TRANSPARENT").color, None);
    assert_eq!((span(&blocks, "SMALL").size, span(&blocks, "HUGE").size), (Some(crate::MIN_FONT_PX), Some(crate::MAX_FONT_PX)));
    let navy = table_with(&blocks, "NAVY-HEAD");
    assert_eq!(navy.background, Some(Rgb(0, 0x33, 0x66)));
}

#[test]
fn hostile_messages_are_bounded() {
    for (name, html) in hostile() {
        assert!(html.len() <= MAX_INPUT * 2, "{name} is {} bytes", html.len());
        let started = std::time::Instant::now();
        let blocks = parse(&html);
        let all = flat(&blocks);
        // Tables are not counted against the cap on blocks: they only hold blocks that were.
        let counted = all.iter().filter(|b| !matches!(b.kind, BlockKind::Table(_))).count();
        assert!(counted <= MAX_BLOCKS, "{name}: {counted} blocks");
        assert!(depth(&blocks) <= MAX_TABLE_DEPTH, "{name}: tables {} deep", depth(&blocks));
        assert!(cells(&blocks) <= MAX_CELLS, "{name}: {} cells", cells(&blocks));
        for b in &blocks {
            check_block(b, name);
        }
        let _ = to_text(&html);
        // Generous: an unoptimised build on a busy machine.
        assert!(started.elapsed().as_secs() < 20, "{name} took {:?}", started.elapsed());
    }
}

#[test]
fn hostile_messages_keep_their_text() {
    let hostile = hostile();
    let text = |name: &str| text_of(&parse(&hostile.iter().find(|(n, _)| *n == name).unwrap_or_else(|| panic!("no {name}")).1));
    assert!(text("deep divs").contains("DEEP-DIVS"));
    assert!(
        text("deep quotes").contains("quote level 2999") && text("deep lists").contains("list level 2999") && text("deep boxes").contains("box 2999")
    );
    assert!(text("deep tables").contains("DEEP-TABLES"));
    let absurd = text("absurd sizes");
    for shown in ["FONT-999", "PX-99999", "PT-1E9", "NEGATIVE", "HUGE-PICTURE", "WIDE-PICTURE", "LAST-NUMBER", "ONE-MORE", "ROMAN", "MARGINS"] {
        assert!(absurd.contains(shown), "{shown} is lost from the absurd sizes");
    }
    assert!(!absurd.contains("EM-TINY"));
    let strange = text("strange text");
    for shown in ["BIDI", "ZALGO", "ZERO-WIDTH", "ASTRAL", "NUL ab", "RTL"] {
        assert!(strange.contains(shown), "{shown} is lost from the strange text");
    }
    assert!(text("one row").matches('x').count() > 1_000);
    assert!(text("many spans").contains("s19999 "));
}

#[test]
fn a_large_newsletter_parses_quickly() {
    let html = large_newsletter(MAX_INPUT);
    assert!(html.len() >= MAX_INPUT && html.len() < MAX_INPUT + 16 * 1024, "{}", html.len());
    let started = std::time::Instant::now();
    let blocks = parse(&html);
    let took = started.elapsed();
    let text = text_of(&blocks);
    assert!(text.contains("Story 1: the harbour at low water") && text.contains("Story 200:"), "the stories are not there");
    assert!(!text.contains("This week: everything, at length."), "the preheader is shown");
    assert!(depth(&blocks) <= 4 && cells(&blocks) <= MAX_CELLS);
    assert!(images(&blocks).iter().all(|b| matches!(&b.kind, BlockKind::Image { src, .. } if !is_embedded(src))));
    // Generous: this is an unoptimised build on whatever machine runs the tests. It takes a few
    // tens of milliseconds when optimised.
    assert!(took.as_secs() < 10, "parsing took {took:?}");
    // Twice the size is cut at the cap, not read.
    let double = format!("{html}{html}");
    assert!(flat(&parse(&double)).len() <= flat(&blocks).len() + 8);
}

/// There is no way to fetch anything from here: the crate has no dependency that could, and its
/// own code names nothing that opens a socket, a file or a process.
#[test]
fn this_crate_has_no_way_to_fetch_anything() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let manifest = std::fs::read_to_string(root.join("Cargo.toml")).unwrap();
    let dependencies: Vec<&str> = manifest
        .lines()
        .skip_while(|l| l.trim() != "[dependencies]")
        .skip(1)
        .take_while(|l| !l.trim_start().starts_with('['))
        .filter_map(|l| l.split('=').next())
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect();
    // `mailcraft-link` only reads an address (and has no dependency of its own): it opens nothing.
    assert_eq!(dependencies, ["mailcraft-link", "serde"], "a new dependency: can it reach the network?");
    let mut files = vec![root.join("src")];
    let mut checked = 0;
    while let Some(path) = files.pop() {
        if path.is_dir() {
            files.extend(std::fs::read_dir(&path).unwrap().map(|e| e.unwrap().path()));
        } else if path.extension().is_some_and(|e| e == "rs") && !path.ends_with("corpus/tests.rs") {
            let code = std::fs::read_to_string(&path).unwrap();
            for forbidden in ["std::net", "TcpStream", "UdpSocket", "std::process", "std::fs", "File::", "reqwest", "ureq", "hyper::", "curl"] {
                assert!(!code.contains(forbidden), "{} names {forbidden}", path.display());
            }
            checked += 1;
        }
    }
    assert!(checked >= 6, "only {checked} source files were found");
}

/// #150: the digest's 600-pixel table is centred and keeps its width; each tile's number has its
/// icon in its line; the footer's pictures are lines of pictures.
#[test]
fn the_access_digest_keeps_its_centred_column_and_its_lines() {
    let blocks = parse(ACCESS_DIGEST);
    let shell = flat(&blocks)
        .into_iter()
        .find(|b| matches!(&b.kind, BlockKind::Table(t) if t.width == Some(Length::Px(600))))
        .unwrap_or_else(|| panic!("no 600-pixel table\n{}", show(&blocks)));
    assert_eq!(shell.align, Align::Center);
    // Its text is where its cells put it (left), and the tiles' text is centred.
    assert_eq!(block(&blocks, "Here is what happened").align, Align::Left);
    assert_eq!(block(&blocks, "Roles activated").align, Align::Center);
    // `<img> <span>3</span>` and `<p><img> 7</p>`: the number goes on in the picture's line.
    for number in ["3", "7"] {
        let all = flat(&blocks);
        let at = all.iter().position(|b| b.text() == number).unwrap_or_else(|| panic!("no {number}\n{}", show(&blocks)));
        assert!(all[at].beside && matches!(all[at - 1].kind, BlockKind::Image { .. }), "{number}\n{}", show(&blocks));
    }
    // The footer's line of linked pictures: one line, no blank lines between them.
    let footer: Vec<&Block> = flat(&blocks)
        .into_iter()
        .filter(|b| matches!(&b.kind, BlockKind::Image { width: Some(20), link: Some(_), .. }) && b.spans.is_empty())
        .collect();
    assert_eq!(footer.len(), 3, "{}", show(&blocks));
    assert!(footer.iter().skip(1).all(|b| b.space == 0 && b.beside), "{footer:?}");
    // The table of roles gives its columns' widths.
    let roles = table_with(&blocks, "Privileged authentication");
    assert_eq!(roles.rows[0].cells.iter().map(|c| c.width).collect::<Vec<_>>(), [Some(Length::Percent(70)), Some(Length::Percent(30))]);
}

/// #150: widths and centring written in `style` lay the payments summary out as a 600-pixel
/// centred card, and a div's `max-width` makes a centred card; nothing is left a narrow column.
#[test]
fn style_widths_and_auto_margins_make_centred_cards() {
    let blocks = parse(STYLE_WIDTH_TABLE);
    let card = flat(&blocks)
        .into_iter()
        .find(|b| matches!(&b.kind, BlockKind::Table(t) if t.width == Some(Length::Px(600))))
        .unwrap_or_else(|| panic!("no 600-pixel table\n{}", show(&blocks)));
    assert_eq!(card.align, Align::Center);
    let roles = table_with(&blocks, "Invoice 1041");
    assert_eq!(roles.rows[0].cells.iter().map(|c| c.width).collect::<Vec<_>>(), [Some(Length::Percent(70)), Some(Length::Percent(30))]);
    let blocks = parse(STYLE_CARD);
    let shown = flat(&blocks);
    let tables: Vec<&Block> = shown.iter().copied().filter(|b| matches!(b.kind, BlockKind::Table(_))).collect();
    assert!(tables.len() >= 2, "{}", show(&blocks));
    // (The page's own background is a box the whole width, on the left.)
    let capped = |b: &Block| matches!(&b.kind, BlockKind::Table(t) if matches!(t.width, Some(Length::Px(_))));
    assert!(tables.iter().copied().filter(|b| capped(b)).all(|b| b.align == Align::Center), "{}", show(&blocks));
    assert!(tables.iter().any(|b| matches!(&b.kind, BlockKind::Table(t) if t.width == Some(Length::Px(600)))), "{}", show(&blocks));
    assert!(tables.iter().any(|b| matches!(&b.kind, BlockKind::Table(t) if t.width == Some(Length::Px(480)))), "{}", show(&blocks));
}

/// #71: a club bulletin written the way a newsletter builder does, with padding, margins, lines
/// of three styles, rounded corners and a line height: all of it is kept, within its limits.
#[test]
fn a_bulletin_keeps_its_padding_margins_lines_corners_and_line_height() {
    let blocks = parse(STYLE_BOXES);
    let all = flat(&blocks);
    let cards: Vec<&Table> = all
        .iter()
        .filter_map(|b| match &b.kind {
            BlockKind::Table(t) if t.outline => Some(t),
            _ => None,
        })
        .collect();
    // The page (solid, 10), the notice (dashed, 8) and the kit swap (dotted, 14, in from the sides).
    let styles: Vec<(LineStyle, u8)> = cards.iter().map(|t| (t.lines[0].style, t.radius)).collect();
    assert!(styles.contains(&(LineStyle::Solid, 10)), "{styles:?}\n{}", show(&blocks));
    assert!(styles.contains(&(LineStyle::Dashed, 8)), "{styles:?}");
    assert!(styles.contains(&(LineStyle::Dotted, 14)), "{styles:?}");
    let swap = all.iter().find(|b| matches!(&b.kind, BlockKind::Table(t) if t.lines[0].style == LineStyle::Dotted)).expect("the kit swap");
    assert_eq!(swap.inset, [40, 40]); // its own 12 and the 28 of the block it is in
    // The notice keeps the room inside it.
    let notice = cards.iter().find(|t| t.lines[0].style == LineStyle::Dashed).expect("the notice");
    assert_eq!(notice.rows[0].cells[0].padding, Some(Insets { top: 12, right: 16, bottom: 12, left: 16 }));
    // The divider is a dashed rule 2 pixels thick; the body text has the line height it asks for.
    let rule = all.iter().find(|b| b.kind == BlockKind::Rule && b.line.width > 0).expect("the divider");
    assert_eq!((rule.line.width, rule.line.style), (2, LineStyle::Dashed));
    let text = all.iter().find(|b| b.text().starts_with("Saturday's walk")).expect("the walk");
    assert_eq!(text.line_height, Some(LineHeight::Scale(170)));
    // The table of leaders gives each cell its room and a line under it.
    let leaders = table_with(&blocks, "Priya Raman");
    assert!(leaders.rows.iter().flat_map(|r| &r.cells).all(|c| c.padding == Some(Insets { top: 10, right: 14, bottom: 10, left: 14 })));
    assert!(leaders.outline && leaders.radius == 6);
}
