//! MailCraft printing: what is printed, laid out as pages by MailCraft itself, and those pages as
//! a PDF that the system then prints.
//!
//! A print is a [`Document`]: pages of positioned text, lines, boxes and pictures, in points
//! from the page's top-left corner. The same document is what the window's Print Preview paints
//! and what [`pdf::write`] turns into a PDF, so the preview is the print. Four kinds are laid
//! out here:
//!
//! - a message in Memo Style ([`memo`]): the owner's name at the top of each page, the header
//!   block (From, Sent, To, Cc, Subject, Attachments), and the body from the display blocks the
//!   reading pane shows (`mailcraft_html`), with page numbers;
//! - a calendar ([`calendar`]): Daily, Work Week, Weekly and Monthly styles over a range of days;
//! - contacts as cards and tasks as a table ([`list`]).
//!
//! Text is set in Inter, which the program already ships (SIL Open Font License), with its
//! Latin, Greek and Cyrillic; a character it has no shape for is printed as the replacement
//! character, never dropped silently and never a panic. JetBrains Mono sets preformatted text.
//! The PDF carries only the shapes it uses.
//!
//! Nothing in a message runs here and nothing is fetched: a picture on the web is printed as a
//! placeholder, and only a picture attached to the message itself (`cid:`), as PNG or JPEG and
//! within size limits, is printed as a picture.
//!
//! What is printed is hostile like everything that comes from a message: lengths are capped
//! (pages, pictures, table sizes, nesting), and nothing here panics on any input.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod calendar;
mod flow;
pub mod font;
pub mod list;
pub mod memo;
pub mod pdf;
pub mod picture;
mod subset;
#[cfg(test)]
mod tests;

pub use font::FontKind;
pub use mailcraft_html::Rgb;

/// The most pages a print has; what would come after is left out, and the last page says so.
pub const MAX_PAGES: usize = 500;

/// A sheet of paper, in points (1/72 inch).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Paper {
    pub width: f32,
    pub height: f32,
}

impl Paper {
    /// US Letter, 8.5 × 11 inches.
    pub const LETTER: Paper = Paper { width: 612.0, height: 792.0 };
    /// ISO A4, 210 × 297 mm.
    pub const A4: Paper = Paper { width: 595.28, height: 841.89 };

    /// `letter` or `a4`.
    pub fn parse(code: &str) -> Option<Paper> {
        match code.trim().to_ascii_lowercase().as_str() {
            "letter" | "us-letter" => Some(Paper::LETTER),
            "a4" => Some(Paper::A4),
            _ => None,
        }
    }
}

impl Default for Paper {
    fn default() -> Self {
        Paper::LETTER
    }
}

/// A line of text set in one style. `y` is the baseline.
#[derive(Clone, Debug, PartialEq)]
pub struct Text {
    pub x: f32,
    pub y: f32,
    pub size: f32,
    pub font: FontKind,
    /// Slanted (the font has no italic of its own; the shapes are sheared).
    pub italic: bool,
    pub color: Rgb,
    /// Only characters the font has (others were replaced when the text was laid out).
    pub text: String,
}

/// Something drawn on a page, in points from the page's top-left corner.
#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Text(Text),
    /// A box, filled, outlined or both.
    Rect {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        fill: Option<Rgb>,
        stroke: Option<Rgb>,
        width: f32,
    },
    /// A straight line.
    Line {
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
        width: f32,
        color: Rgb,
    },
    /// One of the document's pictures, stretched over the box.
    Image {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        picture: usize,
    },
}

impl Item {
    /// The same item moved by `dx`, `dy`.
    pub fn moved(&self, dx: f32, dy: f32) -> Item {
        match self {
            Item::Text(t) => Item::Text(Text { x: t.x + dx, y: t.y + dy, ..t.clone() }),
            Item::Rect { x, y, w, h, fill, stroke, width } => {
                Item::Rect { x: x + dx, y: y + dy, w: *w, h: *h, fill: *fill, stroke: *stroke, width: *width }
            }
            Item::Line { x1, y1, x2, y2, width, color } => {
                Item::Line { x1: x1 + dx, y1: y1 + dy, x2: x2 + dx, y2: y2 + dy, width: *width, color: *color }
            }
            Item::Image { x, y, w, h, picture } => Item::Image { x: x + dx, y: y + dy, w: *w, h: *h, picture: *picture },
        }
    }
}

/// One page.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Page {
    pub items: Vec<Item>,
}

impl Page {
    /// All the page's text, line by line in the order it was laid out (for tests and searching).
    pub fn text(&self) -> String {
        let mut out = String::new();
        let mut last_y = f32::NAN;
        for item in &self.items {
            if let Item::Text(t) = item {
                if !out.is_empty() {
                    out.push(if (t.y - last_y).abs() < 0.5 { ' ' } else { '\n' });
                }
                out.push_str(&t.text);
                last_y = t.y;
            }
        }
        out
    }
}

/// What is printed: pages, and the pictures they show.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Document {
    pub title: String,
    pub paper: Paper,
    pub pages: Vec<Page>,
    pub pictures: Vec<picture::Picture>,
}

impl Document {
    /// The whole document's text, page by page (for tests and searching).
    pub fn text(&self) -> String {
        self.pages.iter().map(Page::text).collect::<Vec<_>>().join("\n\u{c}\n")
    }
}

/// Colours of print: ink on paper, whatever the window's theme.
pub mod ink {
    use super::Rgb;
    pub const TEXT: Rgb = Rgb(0, 0, 0);
    pub const DIM: Rgb = Rgb(90, 90, 90);
    pub const RULE: Rgb = Rgb(0, 0, 0);
    pub const GRID: Rgb = Rgb(170, 170, 170);
    pub const LIGHT: Rgb = Rgb(215, 215, 215);
    pub const SHADE: Rgb = Rgb(236, 236, 236);
    pub const WHITE: Rgb = Rgb(255, 255, 255);
}
