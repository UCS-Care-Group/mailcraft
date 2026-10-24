// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Selecting the text of a message that is being read, and copying it.
//!
//! egui selects the text of its own labels, and that is not used here. Its selection lives in the
//! labels a frame paints: it is dropped as soon as one of its ends is not among them (a message is
//! painted a screenful at a time, so scrolling would lose it), it copies only what was painted, it
//! knows no Select All across labels, and it cannot be told what stands between two texts (a tab
//! between two cells, a blank line between two paragraphs). So a selection here is two places in
//! the message itself ([`Point`]): a thing that can be selected, by its number in reading order,
//! and a character in it. The numbers are the layout's ([`Laid::segs`]) and stay the same when the
//! message is laid out again, at another width or with its pictures.
//!
//! The header's fields (subject, sender, address, date, recipients) are selected the same way,
//! by themselves: as in Outlook, a selection is in the header or in the body, not across both.
//!
//! What is copied is plain text, put together from what the layout knows of each text
//! ([`super::layout::Join`]): a line break between blocks and the blank lines the message has between its
//! paragraphs, a tab between the cells of a table row, list markers in front of their items, a
//! link's words rather than its address, nothing for a picture that is shown and the description
//! of one that is not. Nothing of a message is ever run or followed for it.
//!
//! Everything a message decides is checked where it is used: a place past the end of a text is
//! the end of it, text is cut at character boundaries only, and a selection made in a body that
//! is no longer the one on screen is forgotten.

use std::ops::Range;
use std::sync::Arc;

use egui::text::{CCursor, CCursorRange, CharIndex};
use egui::text_selection::LabelSelectionState;
use egui::{Color32, Galley, PointerButton, Pos2, Rect, Response, Sense, Ui, Vec2, vec2};
use mailcraft_model::{Id, Message};
use serde_json::{Value, json};

use super::Place;
use super::layout::{ItemKind, Laid};
use super::view::Doc;
use crate::MailApp;
use crate::theme::Tokens;

/// How many places a selection is kept for: the reading pane and the message windows.
const MAX_PLACES: usize = 48;
/// The longest address "Copy Link Address" takes.
const MAX_LINK: usize = 8192;
/// While a drag that selects is this near the top or bottom of the body, the body scrolls.
const SCROLL_EDGE: f32 = 18.0;
/// How fast: points a second for each point the pointer is past that, and the limits.
const SCROLL_GAIN: f32 = 14.0;
const SCROLL_SPEED: Range<f32> = 90.0..2400.0;

/// A place in a text: a thing that can be selected (a piece of text or a picture of the body, a
/// field of the header) by its number in reading order, and how far into it, in characters. A
/// picture is one character long.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Point {
    pub seg: usize,
    pub at: usize,
}

impl Point {
    /// Before everything.
    pub const START: Point = Point { seg: 0, at: 0 };
    /// After everything, however much there is, and however much of it is laid out yet.
    pub const END: Point = Point { seg: usize::MAX, at: usize::MAX };
}

/// Which text of a message a selection is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Zone {
    Header,
    Body,
}

/// What is selected: from where the pointer went down to where it went.
#[derive(Clone, Debug)]
pub struct Selection {
    pub zone: Zone,
    /// The end that stays where it is while the other is dragged, or moved with Shift.
    pub anchor: Point,
    pub head: Point,
    /// The body it was made in, once that is known: it does not outlive it.
    doc: Option<Arc<Doc>>,
}

impl Selection {
    fn new(zone: Zone, anchor: Point, head: Point) -> Selection {
        Selection { zone, anchor, head, doc: None }
    }
}

/// A text a selection is made in: the body of a message as it is laid out, or its header.
pub trait Surface {
    /// How many things there are to select.
    fn count(&self) -> usize;
    /// How long the `seg`th is: its characters, or 1 for a picture.
    fn len(&self, seg: usize) -> usize;
    /// Its text, when it is one.
    fn text(&self, seg: usize) -> Option<&str>;
    /// Whether it goes on in the paragraph of the one before it.
    fn continues(&self, seg: usize) -> bool;
    /// Write characters `part` of it to `out` as a copy has them, after what stands between it
    /// and what was written before (nothing when it is the `first`). Whether anything was written.
    fn write(&self, out: &mut String, seg: usize, part: Range<usize>, first: bool) -> bool;
}

/// `point` as a place that is there: one past the end is the end. `None` when there is nothing.
pub fn clamp<S: Surface + ?Sized>(surface: &S, point: Point) -> Option<Point> {
    let last = surface.count().checked_sub(1)?;
    if point.seg > last {
        return Some(Point { seg: last, at: surface.len(last) });
    }
    Some(Point { seg: point.seg, at: point.at.min(surface.len(point.seg)) })
}

/// The two ends of a selection in reading order, as they are in `surface`; `None` when they
/// are the same place.
pub fn range<S: Surface + ?Sized>(surface: &S, selection: &Selection) -> Option<(Point, Point)> {
    let (a, b) = (clamp(surface, selection.anchor)?, clamp(surface, selection.head)?);
    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
    (lo < hi).then_some((lo, hi))
}

/// The characters of the `seg`th thing, `len` long, that lie between `lo` and `hi`.
pub fn part(lo: Point, hi: Point, seg: usize, len: usize) -> Option<Range<usize>> {
    if seg < lo.seg || seg > hi.seg {
        return None;
    }
    let from = if seg == lo.seg { lo.at.min(len) } else { 0 };
    let to = if seg == hi.seg { hi.at.min(len) } else { len };
    (from < to).then_some(from..to)
}

/// What lies between `lo` and `hi` as plain text.
pub fn copied<S: Surface + ?Sized>(surface: &S, lo: Point, hi: Point) -> String {
    let mut out = String::new();
    let Some(last) = surface.count().checked_sub(1) else { return out };
    let mut first = true;
    for seg in lo.seg..=hi.seg.min(last) {
        let Some(part) = part(lo, hi, seg, surface.len(seg)) else { continue };
        if surface.write(&mut out, seg, part, first) {
            first = false;
        }
    }
    out
}

/// Characters `from..to` of `text`: cut at character boundaries, and at the end of the text.
pub fn char_slice(text: &str, from: usize, to: usize) -> &str {
    if from >= to {
        return "";
    }
    let mut bounds = text.char_indices().map(|(at, _)| at).chain(std::iter::once(text.len()));
    let Some(start) = bounds.nth(from) else { return "" };
    // `nth` counts on from where the last one stopped.
    let end = bounds.nth(to - from - 1).unwrap_or(text.len());
    text.get(start..end).unwrap_or("")
}

/// What a character is to a double click.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Word,
    Space,
    /// A sign: each is a word of its own.
    Other,
}

fn class(c: char) -> Class {
    if c.is_alphanumeric() || c == '_' {
        Class::Word
    } else if c.is_whitespace() {
        Class::Space
    } else {
        Class::Other
    }
}

/// Whether a character belongs to the one before it: an accent, a joiner, a variation selector.
fn attaches(c: char) -> bool {
    matches!(c, '\u{300}'..='\u{36f}' | '\u{483}'..='\u{489}' | '\u{591}'..='\u{5bd}' | '\u{610}'..='\u{61a}' | '\u{64b}'..='\u{65f}'
        | '\u{1ab0}'..='\u{1aff}' | '\u{1dc0}'..='\u{1dff}' | '\u{200c}' | '\u{200d}' | '\u{20d0}'..='\u{20ff}' | '\u{fe00}'..='\u{fe0f}'
        | '\u{fe20}'..='\u{fe2f}' | '\u{1f3fb}'..='\u{1f3ff}' | '\u{e0100}'..='\u{e01ef}')
}

/// The word of `text` that character `at` is in, as a range of characters: letters and digits
/// with what holds them together (an apostrophe between letters, a point or comma between
/// digits), or a run of white space, or a single sign.
pub fn word_range(text: &str, at: usize) -> Range<usize> {
    let mut start = 0;
    let mut before: Option<(Class, char)> = None;
    let mut chars = text.chars().enumerate().peekable();
    let mut total = 0;
    while let Some((i, c)) = chars.next() {
        total = i + 1;
        let next = chars.peek().map(|(_, n)| *n);
        // An accent goes with its letter, and what a joiner joins with what is before it.
        let glued = before.is_some_and(|(_, b)| attaches(c) || b == '\u{200d}');
        let kind = match before {
            Some((kind, _)) if glued => kind,
            // A sign inside a word or a number is part of it.
            Some((Class::Word, b)) if next.is_some_and(|n| inside(b, c, n)) => Class::Word,
            _ => class(c),
        };
        let breaks = match before {
            None => false,
            Some((was, _)) => !glued && (was != kind || kind == Class::Other),
        };
        if breaks {
            if i > at {
                return start..i;
            }
            start = i;
        }
        before = Some((kind, c));
    }
    start..total
}

/// Whether sign `c` between `b` and `n` is inside a word: `don't`, `83.20`, `1,000`.
fn inside(b: char, c: char, n: char) -> bool {
    match c {
        '\'' | '\u{2019}' => b.is_alphabetic() && n.is_alphabetic(),
        '.' | ',' => b.is_numeric() && n.is_numeric(),
        _ => false,
    }
}

/// The word at `point`, or the whole of a picture.
fn word_at<S: Surface + ?Sized>(surface: &S, point: Point) -> (Point, Point) {
    let seg = point.seg;
    match surface.text(seg) {
        Some(text) => {
            let word = word_range(text, point.at);
            (Point { seg, at: word.start }, Point { seg, at: word.end.min(surface.len(seg)) })
        }
        None => (Point { seg, at: 0 }, Point { seg, at: surface.len(seg) }),
    }
}

/// The paragraph `point` is in: its text, with the other pieces of it when it was cut for its length.
fn paragraph_at<S: Surface + ?Sized>(surface: &S, point: Point) -> (Point, Point) {
    let (mut first, mut last) = (point.seg, point.seg);
    while first > 0 && surface.continues(first) {
        first -= 1;
    }
    while last.saturating_add(1) < surface.count() && surface.continues(last + 1) {
        last += 1;
    }
    (Point { seg: first, at: 0 }, Point { seg: last, at: surface.len(last) })
}

// ---- the body ----

impl Surface for Laid {
    fn count(&self) -> usize {
        self.segs.len()
    }

    fn len(&self, seg: usize) -> usize {
        match self.segment(seg).map(|item| &item.kind) {
            Some(ItemKind::Text(text)) => text.chars as usize,
            Some(ItemKind::Picture(_) | ItemKind::Placeholder(_)) => 1,
            _ => 0,
        }
    }

    fn text(&self, seg: usize) -> Option<&str> {
        match self.segment(seg).map(|item| &item.kind) {
            Some(ItemKind::Text(text)) => Some(&text.job.text),
            _ => None,
        }
    }

    fn continues(&self, seg: usize) -> bool {
        seg > 0 && matches!(self.segment(seg).map(|item| &item.kind), Some(ItemKind::Text(text)) if text.join.cont)
    }

    fn write(&self, out: &mut String, seg: usize, part: Range<usize>, first: bool) -> bool {
        match self.segment(seg).map(|item| &item.kind) {
            Some(ItemKind::Text(text)) => {
                let piece = char_slice(&text.job.text, part.start, part.end);
                if piece.is_empty() {
                    return false;
                }
                if !first {
                    text.join.write(out);
                }
                // A list item copied from its start has its marker.
                if part.start == 0 {
                    out.push_str(&text.lead);
                }
                out.push_str(piece);
                true
            }
            // A picture that is not shown is copied as what it says it shows, when it is whole
            // in the selection; one that is shown is not text.
            Some(ItemKind::Placeholder(stand_in)) if stand_in.described && part.start == 0 && part.end >= 1 => {
                if !first {
                    stand_in.join.write(out);
                }
                out.push_str(&stand_in.label);
                true
            }
            _ => false,
        }
    }
}

/// A place in a text under the pointer.
#[derive(Clone, Copy, Debug)]
pub struct Hit {
    /// The gap between two characters that is nearest: where a selection begins or ends.
    pub point: Point,
    /// The character the pointer is on, when it is on one: what a double click takes the word of.
    pub under: Option<usize>,
}

/// The place in a laid-out body nearest `at` (a point from its top left): in the text or the
/// picture it is on, else in the nearest one.
pub fn hit(ctx: &egui::Context, laid: &Laid, at: Pos2) -> Option<Hit> {
    let mut best: Option<(f32, &super::layout::Item)> = None;
    for index in &laid.segs {
        let Some(item) = laid.items.get(*index as usize) else { continue };
        let distance = item.rect.distance_sq_to_pos(at);
        if best.is_none_or(|(least, _)| distance < least) {
            best = Some((distance, item));
            if distance <= 0.0 {
                break;
            }
        }
    }
    let (_, item) = best?;
    let seg = item.seg as usize;
    match &item.kind {
        ItemKind::Text(text) => {
            let galley = ctx.fonts_mut(|f| f.layout_job((*text.job).clone()));
            let from = at - egui::pos2(item.rect.min.x + text.anchor, item.rect.min.y);
            // The laid-out text counts by its glyphs, and a script this system has no font for is
            // laid out with more of them than it has characters: neither count may pass the text's.
            let chars = text.chars as usize;
            let nearest = galley.cursor_from_pos(from).index.0.min(chars);
            let under = super::view::char_at(&galley, from).filter(|under| *under < chars);
            Some(Hit { point: Point { seg, at: nearest }, under })
        }
        ItemKind::Picture(_) | ItemKind::Placeholder(_) => {
            Some(Hit { point: Point { seg, at: usize::from(at.x >= item.rect.center().x) }, under: None })
        }
        _ => None,
    }
}

// ---- the header ----

/// A field of a header as it was last shown.
#[derive(Clone)]
pub struct HeaderField {
    galley: Arc<Galley>,
    /// What a copy writes between it and the field before.
    join: &'static str,
}

impl HeaderField {
    /// How many characters of it are on screen: all of them, unless it is cut short.
    fn shown(&self) -> usize {
        self.galley.end().index.0
    }
}

impl Surface for [HeaderField] {
    fn count(&self) -> usize {
        self.len()
    }

    fn len(&self, seg: usize) -> usize {
        self.get(seg).map_or(0, HeaderField::shown)
    }

    fn text(&self, seg: usize) -> Option<&str> {
        self.get(seg).map(|field| field.galley.text())
    }

    fn continues(&self, _seg: usize) -> bool {
        false
    }

    fn write(&self, out: &mut String, seg: usize, part: Range<usize>, first: bool) -> bool {
        let Some(field) = self.get(seg) else { return false };
        // A field that is cut short with an ellipsis is copied whole when it is selected to its end.
        let to = if field.galley.elided && part.end >= field.shown() { usize::MAX } else { part.end };
        let piece = char_slice(field.galley.text(), part.start, to);
        if piece.is_empty() {
            return false;
        }
        if !first {
            out.push_str(field.join);
        }
        out.push_str(piece);
        true
    }
}

/// The fields of a header, in the order they are read and copied in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Field {
    Subject,
    Sender,
    Address,
    Date,
    To,
    Cc,
    Bcc,
}

impl Field {
    fn join(self) -> &'static str {
        match self {
            // The address stands beside the name.
            Field::Address => " ",
            _ => "\n",
        }
    }
}

/// A field as it is laid out in this frame.
struct Shown {
    field: Field,
    /// Where its text's origin is.
    pos: Pos2,
    galley: Arc<Galley>,
    response: Response,
}

/// The fields of a header as they are laid out, to be painted once the pointer has had its say
/// ([`header`]).
#[derive(Default)]
pub(super) struct Fields {
    shown: Vec<Shown>,
}

impl Fields {
    /// Lay `label` out where a label goes, as the text of `field`. It is not painted yet.
    pub(super) fn add(&mut self, ui: &mut Ui, field: Field, label: egui::Label) {
        let (pos, galley, response) = label.selectable(false).sense(sense(ui)).layout_in_ui(ui);
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, ui.is_enabled(), galley.text()));
        // What is cut short is shown whole when the pointer rests on it.
        let response = if galley.elided { response.on_hover_text(galley.text().to_string()) } else { response };
        self.shown.push(Shown { field, pos, galley, response });
    }
}

/// What a text that can be selected senses: clicks, and drags where a drag is not how the
/// screen is scrolled. It takes no part in Tab's round.
pub(super) fn sense(ui: &Ui) -> Sense {
    if ui.input(|i| i.has_touch_screen()) { Sense::CLICK } else { Sense::CLICK | Sense::DRAG }
}

/// The place in the header's fields nearest `pos`.
fn hit_fields(shown: &[Shown], pos: Pos2) -> Option<Hit> {
    let mut best: Option<(f32, usize)> = None;
    for (seg, field) in shown.iter().enumerate() {
        let distance = field.response.rect.distance_sq_to_pos(pos);
        if best.is_none_or(|(least, _)| distance < least) {
            best = Some((distance, seg));
        }
    }
    let (_, seg) = best?;
    let field = shown.get(seg)?;
    let from = pos - field.pos;
    Some(Hit { point: Point { seg, at: field.galley.cursor_from_pos(from).index.0 }, under: super::view::char_at(&field.galley, from) })
}

// ---- what is selected where ----

/// What is selected in each place a message is read in, and which of them the keys are for.
#[derive(Default)]
pub struct Selections {
    places: Vec<PlaceState>,
    /// The place the pointer last went down in.
    focus: Option<Place>,
}

/// A place a message is read in: what is selected there, and what the pointer is doing.
pub struct PlaceState {
    place: Place,
    message: Id,
    /// The frame it was last shown in, and the window.
    frame: u64,
    viewport: egui::ViewportId,
    /// The pointer was over it then.
    hovered: bool,
    selection: Option<Selection>,
    /// Where the pointer went down, for a drag to start from.
    pressed: Option<(Zone, Point)>,
    /// A drag is selecting.
    dragging: Option<Zone>,
    /// The pointer went down with Shift on what was selected: it moves the end of that.
    extending: bool,
    /// The header's fields as they were last shown.
    header: Vec<HeaderField>,
    /// The link that was under the pointer when the body's menu was asked for.
    menu_link: Option<Arc<str>>,
    /// A copy was asked for of a selection that reaches the end of a message that was still
    /// being laid out: it is made when the layout is whole.
    copy_when_laid: bool,
}

impl PlaceState {
    fn new(place: Place) -> PlaceState {
        PlaceState {
            place,
            message: 0,
            frame: 0,
            viewport: egui::ViewportId::ROOT,
            hovered: false,
            selection: None,
            pressed: None,
            dragging: None,
            extending: false,
            header: Vec::new(),
            menu_link: None,
            copy_when_laid: false,
        }
    }

    /// Another message is shown here now.
    fn reset(&mut self, message: Id) {
        *self = PlaceState { message, frame: self.frame, viewport: self.viewport, ..PlaceState::new(self.place) };
    }

    /// Something that could be copied is selected (as far as can be told without the text).
    fn selected(&self) -> bool {
        self.selection.as_ref().is_some_and(|s| s.anchor != s.head)
    }

    /// The body that is on screen. A selection that was made in another one is gone with it; one
    /// that is new is this body's from now on. A copy that waited for the layout is due when it
    /// is `done`.
    pub(super) fn bind(&mut self, doc: &Arc<Doc>, done: bool) -> Option<Action> {
        if let Some(selection) = &mut self.selection
            && selection.zone == Zone::Body
        {
            match &selection.doc {
                Some(of) if !Arc::ptr_eq(of, doc) => {
                    self.selection = None;
                    self.copy_when_laid = false;
                }
                Some(_) => {}
                None => selection.doc = Some(doc.clone()),
            }
        }
        (done && std::mem::take(&mut self.copy_when_laid)).then_some(Action::Copy)
    }

    /// The ends of what is selected in the body, in reading order.
    pub(super) fn body_range(&self, laid: &Laid) -> Option<(Point, Point)> {
        self.selection.as_ref().filter(|s| s.zone == Zone::Body).and_then(|s| range(laid, s))
    }

    /// A drag is selecting in the body.
    pub(super) fn dragging_body(&self) -> bool {
        self.dragging == Some(Zone::Body)
    }

    pub(super) fn menu_link(&self) -> Option<Arc<str>> {
        self.menu_link.clone()
    }

    pub(super) fn set_menu_link(&mut self, link: Option<Arc<str>>) {
        self.menu_link = link;
    }

    /// The message that is shown has no body to select in: nothing of it is selected, and no
    /// copy of it is waited for.
    pub(super) fn no_body(&mut self) {
        if self.selection.as_ref().is_some_and(|s| s.zone == Zone::Body) {
            self.selection = None;
        }
        self.copy_when_laid = false;
    }

    /// Do to the selection what the pointer did in the text of `zone`: a drag selects from where
    /// it went down, with Shift it moves the end of what is selected; two clicks take a word,
    /// three a paragraph; one click lets go of everything.
    fn follow<S: Surface + ?Sized>(&mut self, zone: Zone, surface: &S, gesture: &Gesture, hit: impl Fn(Pos2) -> Option<Hit>) {
        if let Some(pos) = gesture.pressed {
            self.pressed = hit(pos).map(|hit| (zone, hit.point));
            self.dragging = None;
            self.extending = gesture.shift && self.selection.as_ref().is_some_and(|s| s.zone == zone);
            if self.extending
                && let (Some(selection), Some((_, to))) = (&mut self.selection, self.pressed)
            {
                selection.head = to;
            }
        }
        match gesture.dragged.and_then(&hit) {
            Some(hit) => {
                if self.dragging != Some(zone) {
                    self.dragging = Some(zone);
                    if !self.extending {
                        let from = self.pressed.filter(|(of, _)| *of == zone).map_or(hit.point, |(_, point)| point);
                        self.selection = Some(Selection::new(zone, from, from));
                    }
                }
                if let Some(selection) = self.selection.as_mut().filter(|s| s.zone == zone) {
                    selection.head = hit.point;
                }
            }
            None if self.dragging == Some(zone) && gesture.dragged.is_none() => self.dragging = None,
            None => {}
        }
        if let Some((pos, clicks)) = gesture.clicked {
            self.dragging = None;
            let ends = match (clicks, hit(pos)) {
                (0 | 1, _) | (_, None) => None,
                (2, Some(hit)) => Some(word_at(surface, Point { seg: hit.point.seg, at: hit.under.unwrap_or(hit.point.at) })),
                (_, Some(hit)) => Some(paragraph_at(surface, hit.point)),
            };
            match ends {
                Some((from, to)) => self.selection = Some(Selection::new(zone, from, to)),
                // A click with Shift has moved the end already.
                None if self.extending || gesture.shift => {}
                None => self.selection = None,
            }
        }
    }
}

impl Selections {
    /// The state of `place`, made when it is not there yet.
    fn open(&mut self, place: Place) -> Option<&mut PlaceState> {
        if !self.places.iter().any(|s| s.place == place) {
            // The places that were not shown for longest make room.
            if self.places.len() >= MAX_PLACES
                && let Some(oldest) = self.places.iter().enumerate().min_by_key(|(_, s)| s.frame).map(|(i, _)| i)
            {
                self.places.swap_remove(oldest);
            }
            self.places.push(PlaceState::new(place));
        }
        self.state(place)
    }

    pub(super) fn state(&mut self, place: Place) -> Option<&mut PlaceState> {
        self.places.iter_mut().find(|s| s.place == place)
    }

    /// Whether something is selected in a message, wherever.
    pub fn any(&self) -> bool {
        self.places.iter().any(PlaceState::selected)
    }

    /// The places that are on screen in a window: shown in this frame or the one before.
    fn live(&self, viewport: egui::ViewportId, frame: u64) -> impl Iterator<Item = &PlaceState> {
        self.places.iter().filter(move |s| s.viewport == viewport && s.frame.saturating_add(1) >= frame)
    }

    /// The place the keys are for in a window: the one the pointer last went down in, else the
    /// one it is over.
    fn key_target(&self, viewport: egui::ViewportId, frame: u64) -> Option<Place> {
        if let Some(focus) = self.focus
            && self.live(viewport, frame).any(|s| s.place == focus)
        {
            return Some(focus);
        }
        if let Some(over) = self.live(viewport, frame).find(|s| s.hovered) {
            return Some(over.place);
        }
        // A message's own window of the system shows that message and nothing else: its keys
        // are the message's wherever the pointer is.
        let mut there = self.live(viewport, frame).filter(|_| viewport != egui::ViewportId::ROOT);
        match (there.next(), there.next()) {
            (Some(only), None) => Some(only.place),
            _ => None,
        }
    }

    /// The place whose selection Ctrl+C copies: the one the pointer last went down in; when it
    /// went down somewhere else since, the one it is over, or the only one that has a selection.
    /// `labels`: one of egui's labels has a selection of its own, which is the newer one then.
    fn copy_target(&self, viewport: egui::ViewportId, frame: u64, labels: bool) -> Option<Place> {
        if let Some(focus) = self.focus
            && let Some(state) = self.live(viewport, frame).find(|s| s.place == focus)
        {
            return state.selected().then_some(focus);
        }
        if labels {
            return None;
        }
        if let Some(over) = self.live(viewport, frame).find(|s| s.hovered && s.selected()) {
            return Some(over.place);
        }
        let mut with = self.live(viewport, frame).filter(|s| s.selected());
        match (with.next(), with.next()) {
            (Some(only), None) => Some(only.place),
            _ => None,
        }
    }

    /// What is selected at `place`, and where the keys go, for tests.
    #[cfg(test)]
    pub(crate) fn at(&self, place: Place) -> Option<(Zone, Point, Point)> {
        self.places.iter().find(|s| s.place == place).and_then(|s| s.selection.as_ref()).map(|s| (s.zone, s.anchor, s.head))
    }

    #[cfg(test)]
    pub(crate) fn focus(&self) -> Option<Place> {
        self.focus
    }
}

/// What the pointer did in this frame to a text that can be selected.
struct Gesture {
    /// The first button went down on it, here.
    pressed: Option<Pos2>,
    /// It is being dragged from it, and is here now.
    dragged: Option<Pos2>,
    /// A click on it ended here: the first, second or third in a row.
    clicked: Option<(Pos2, u8)>,
    /// Its menu was asked for here.
    menu: Option<Pos2>,
    shift: bool,
}

impl Gesture {
    fn of<'a>(ui: &Ui, responses: impl Iterator<Item = &'a Response>) -> Gesture {
        // Shift as the window has it, or as the button's own event says (a script's click).
        let with_shift = |e: &egui::Event| matches!(e, egui::Event::PointerButton { modifiers, .. } if modifiers.shift);
        let (down, origin, now, shift) = ui.input(|i| {
            (i.pointer.primary_pressed(), i.pointer.press_origin(), i.pointer.latest_pos(), i.modifiers.shift || i.events.iter().any(with_shift))
        });
        let mut gesture = Gesture { pressed: None, dragged: None, clicked: None, menu: None, shift };
        for response in responses {
            if down && response.is_pointer_button_down_on() {
                gesture.pressed = origin.or(response.interact_pointer_pos());
            }
            if response.dragged_by(PointerButton::Primary) {
                gesture.dragged = response.interact_pointer_pos().or(now);
            }
            if response.clicked() {
                let clicks = if response.triple_clicked() {
                    3
                } else if response.double_clicked() {
                    2
                } else {
                    1
                };
                gesture.clicked = response.interact_pointer_pos().or(now).map(|pos| (pos, clicks));
            }
            if response.secondary_clicked() {
                gesture.menu = response.interact_pointer_pos().or(now);
            }
        }
        gesture
    }
}

/// What a menu or a key asks for; it is done through a command, once nothing is borrowed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Action {
    Copy,
    SelectAll,
    CopyLink(Arc<str>),
}

/// `place` as a command's parameters.
fn params(place: Place) -> Value {
    match place {
        Place::Pane => json!({"pane": true}),
        Place::Window(id) => json!({"window": id}),
        Place::Quote => json!({"quote": true}),
        Place::Preview => json!({"preview": true}),
    }
}

pub(super) fn act(app: &mut MailApp, place: Place, action: Action) {
    let _ = match action {
        Action::Copy => app.run("ui.copySelection", params(place)),
        Action::SelectAll => app.run("ui.selectAllInMessage", params(place)),
        Action::CopyLink(url) => app.run("ui.copyLinkAddress", json!({"url": &*url})),
    };
}

/// A message is about to be shown at `place`. What was selected there in another message is
/// forgotten; where the pointer goes down says whom the keys are for; and the keys that are
/// about a selection are acted on: Ctrl+C copies it, Escape lets go of it.
pub(super) fn begin(app: &mut MailApp, ui: &Ui, m: &Message, place: Place) {
    let ctx = ui.ctx();
    let frame = super::images::frame(ctx);
    let viewport = ctx.viewport_id();
    let inside = ui.rect_contains_pointer(ui.available_rect_before_wrap());
    let menus = egui::Popup::is_any_open(ctx);
    // Ctrl+A (Cmd+A on a Mac), by the modifiers the key's own event names.
    let select_all = |e: &egui::Event| matches!(e, egui::Event::Key { key: egui::Key::A, pressed: true, modifiers, .. } if modifiers.command && !modifiers.alt && !modifiers.shift);
    let (pressed, copy, escape, all) = ui.input(|i| {
        (
            i.pointer.any_pressed(),
            i.events.iter().any(|e| matches!(e, egui::Event::Copy)),
            // A press, not a held one's repeats: those come after the search box has had it (#365).
            i.events.iter().any(|e| matches!(e, egui::Event::Key { key: egui::Key::Escape, pressed: true, repeat: false, .. })),
            i.events.iter().any(select_all),
        )
    });
    let selections = &mut app.text_selection;
    let had_keys = selections.focus == Some(place);
    let Some(state) = selections.open(place) else { return };
    // Another message, or the same one after it was away: nothing of before holds.
    let fresh = state.message != m.id || state.frame.saturating_add(1) < frame;
    if fresh {
        state.reset(m.id);
    }
    (state.frame, state.viewport, state.hovered) = (frame, viewport, inside);
    if fresh && had_keys {
        selections.focus = None;
    }
    // A press on a menu's item is not a press somewhere else.
    if pressed && !menus {
        if inside {
            selections.focus = Some(place);
        } else if selections.focus == Some(place) {
            selections.focus = None;
        }
    }
    // A text field that has the keyboard has these keys too, for its own text.
    if ctx.text_edit_focused() {
        return;
    }
    let mut actions = Vec::new();
    if copy {
        let labels = ctx.with_plugin(|labels: &mut LabelSelectionState| labels.has_selection()).unwrap_or(false);
        if selections.copy_target(viewport, frame, labels) == Some(place) {
            actions.push(Action::Copy);
        }
    }
    if escape
        && !menus
        && let Some(state) = selections.state(place)
    {
        (state.selection, state.dragging, state.copy_when_laid) = (None, None, false);
    }
    // In the main window the shortcuts see Ctrl+A first and send it here when it is for a
    // message ([`select_all_params`]); a window of the system has its keys to itself.
    if all && viewport != egui::ViewportId::ROOT && selections.key_target(viewport, frame) == Some(place) {
        actions.push(Action::SelectAll);
    }
    for action in actions {
        act(app, place, action);
    }
}

/// Where Ctrl+A selects the text of a message rather than the messages of the list: the place
/// the pointer last went down in, or is over. As the parameters of `ui.selectAllInMessage`.
pub(crate) fn select_all_params(app: &MailApp, ctx: &egui::Context) -> Option<Value> {
    app.text_selection.key_target(ctx.viewport_id(), super::images::frame(ctx)).map(params)
}

/// The colours of selected text: the window's accent with the colour that is read on it, which
/// can be read whatever the message painted behind its text.
pub(super) struct Marks {
    visuals: egui::Visuals,
    /// What is laid over a picture that is selected.
    pub(super) veil: Color32,
}

impl Marks {
    pub(super) fn new(ui: &Ui, t: &Tokens) -> Marks {
        let mut visuals = ui.visuals().clone();
        visuals.selection.bg_fill = t.accent;
        visuals.selection.stroke.color = t.on_accent;
        Marks { visuals, veil: t.accent.gamma_multiply(0.45) }
    }

    /// Mark characters `part` of a laid-out text as selected.
    pub(super) fn text(&self, galley: &mut Arc<Galley>, part: Range<usize>) {
        let end = galley.end().index.0;
        let (from, to) = (part.start.min(end), part.end.min(end));
        if from >= to {
            return;
        }
        let range = CCursorRange::two(CCursor { index: CharIndex(from), prefer_next_row: true }, CCursor::new(to));
        egui::text_selection::visuals::paint_text_selection(galley, &self.visuals, &range, None);
    }
}

/// The items of a text's menu: Copy, and for the body Select All and, on a link, Copy Link
/// Address. What was chosen.
pub(super) fn menu(ui: &mut Ui, can_copy: bool, body: bool, link: Option<&Arc<str>>) -> Option<Action> {
    let item = |ui: &mut Ui, enabled: bool, label: &str, key: &str| {
        let button = egui::Button::new(tl!(label)).min_size(vec2(190.0, 0.0));
        let button = if key.is_empty() { button } else { button.shortcut_text(crate::widgets::key_text(key)) };
        ui.add_enabled(enabled, button).clicked()
    };
    let mut chosen = None;
    if item(ui, can_copy, "Copy", "Mod+C") {
        chosen = Some(Action::Copy);
    }
    if body && item(ui, true, "Select All", "Mod+A") {
        chosen = Some(Action::SelectAll);
    }
    if let Some(url) = link {
        ui.separator();
        if item(ui, true, "Copy Link Address", "") {
            chosen = Some(Action::CopyLink(url.clone()));
        }
    }
    if chosen.is_some() {
        ui.close();
    }
    chosen
}

/// The pointer in the body: follow it with the selection, and scroll when a drag that selects
/// comes to the top or bottom of what is in view. `origin` is where the message's top left is,
/// `response` senses the whole of the body that is in view.
pub(super) fn body(ui: &Ui, response: &Response, laid: &Laid, origin: Pos2, state: &mut PlaceState) {
    let ctx = ui.ctx();
    let gesture = Gesture::of(ui, std::iter::once(response));
    state.follow(Zone::Body, laid, &gesture, |pos| hit(ctx, laid, (pos - origin).to_pos2()));
    let Some(pos) = gesture.dragged.filter(|_| state.dragging_body()) else { return };
    let view = response.rect;
    let dt = ui.input(|i| i.stable_dt).clamp(0.0, 0.1);
    // How far the pointer is past where scrolling begins: inside the edge up and down, outside
    // the body sideways (a line is selected to its end without the body moving under it).
    let past = |p: f32, low: f32, high: f32| {
        if p < low {
            p - low
        } else if p > high {
            p - high
        } else {
            0.0
        }
    };
    let over = vec2(past(pos.x, view.left(), view.right()), past(pos.y, view.top() + SCROLL_EDGE, view.bottom() - SCROLL_EDGE));
    if over == Vec2::ZERO || !over.is_finite() {
        return;
    }
    // Content moves against the pointer: dragging past the bottom brings up what is below.
    let step =
        |over: f32| if over == 0.0 { 0.0 } else { -over.signum() * (over.abs() * SCROLL_GAIN).clamp(SCROLL_SPEED.start, SCROLL_SPEED.end) * dt };
    ui.scroll_with_delta_animation(vec2(step(over.x), step(over.y)), egui::style::ScrollAnimation::none());
    ctx.request_repaint();
}

/// The header's fields, laid out: follow the pointer in them, and paint them with what is
/// selected. `around` is the whole header: a press in it that lands on none of its texts lets
/// go of the selection.
pub(super) fn header(app: &mut MailApp, ui: &Ui, place: Place, mut fields: Fields, around: Rect) {
    fields.shown.sort_by_key(|shown| shown.field);
    let shown = &fields.shown;
    let t = Tokens::get(ui.ctx());
    let gesture = Gesture::of(ui, shown.iter().map(|field| &field.response));
    let pressed_elsewhere = gesture.pressed.is_none()
        && ui.input(|i| i.pointer.primary_pressed())
        && ui.rect_contains_pointer(around)
        && !egui::Popup::is_any_open(ui.ctx());
    let mut selected = None;
    let mut dragging = false;
    if let Some(state) = app.text_selection.state(place) {
        let header: Vec<HeaderField> = shown.iter().map(|field| HeaderField { galley: field.galley.clone(), join: field.field.join() }).collect();
        if pressed_elsewhere {
            (state.selection, state.dragging) = (None, None);
        }
        state.follow(Zone::Header, header.as_slice(), &gesture, |pos| hit_fields(shown, pos));
        let of_header = |state: &PlaceState| state.selection.as_ref().filter(|s| s.zone == Zone::Header).and_then(|s| range(header.as_slice(), s));
        // The menu's Copy is about what is selected: asked for on a field with nothing selected,
        // it is that field.
        if let Some(hit) = gesture.menu.and_then(|pos| hit_fields(shown, pos))
            && of_header(state).is_none()
        {
            let seg = hit.point.seg;
            state.selection = Some(Selection::new(Zone::Header, Point { seg, at: 0 }, Point { seg, at: Surface::len(header.as_slice(), seg) }));
        }
        selected = of_header(state);
        dragging = state.dragging == Some(Zone::Header);
        state.header = header;
    }
    let marks = selected.map(|_| Marks::new(ui, &t));
    let mut action = None;
    for (seg, field) in shown.iter().enumerate() {
        let mut galley = field.galley.clone();
        if let (Some(marks), Some((lo, hi))) = (&marks, selected)
            && let Some(part) = part(lo, hi, seg, galley.end().index.0)
        {
            marks.text(&mut galley, part);
        }
        ui.painter().galley(field.pos, galley, ui.visuals().text_color());
        if field.response.hovered() || dragging {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
        }
        field.response.context_menu(|ui| {
            if let Some(chosen) = menu(ui, selected.is_some(), false, None) {
                action = Some(chosen);
            }
        });
    }
    if let Some(action) = action {
        act(app, place, action);
    }
}

// ---- commands ----

/// The window commands that are about selected text (`UI_COMMANDS` lists them).
pub(crate) fn ui_command(app: &mut MailApp, id: &str, p: &Value) -> Option<Result<Value, String>> {
    Some(match id {
        "ui.copySelection" => copy(app, p),
        "ui.selectAllInMessage" => select_all(app, p),
        "ui.copyLinkAddress" => copy_link(app, p),
        _ => return None,
    })
}

/// The place a command is about: the one it names, else the one the keys are for, else the
/// reading pane.
fn place_of(app: &MailApp, p: &Value) -> Result<Place, String> {
    match p.get("window") {
        None | Some(Value::Null) if p.get("pane").and_then(Value::as_bool) == Some(true) => Ok(Place::Pane),
        None | Some(Value::Null) => Ok(app.text_selection.focus.unwrap_or(Place::Pane)),
        Some(id) => id.as_u64().map(Place::Window).ok_or_else(|| "`window` is a message window's number".to_string()),
    }
}

/// Text as the clipboard of this system has it: on Windows a line ends with a carriage return
/// and a line feed, as every program there expects of plain text.
fn for_clipboard(text: &str) -> String {
    let text = text.replace("\r\n", "\n");
    if cfg!(target_os = "windows") { text.replace('\n', "\r\n") } else { text }
}

/// What is selected at `place` as plain text. `None` when it reaches the end of a body that is
/// not laid out to its end yet; an empty text when nothing is selected.
fn selected_text(ctx: &egui::Context, state: &PlaceState) -> Option<String> {
    let Some(selection) = &state.selection else { return Some(String::new()) };
    match selection.zone {
        Zone::Header => {
            let header = state.header.as_slice();
            Some(range(header, selection).map(|(lo, hi)| copied(header, lo, hi)).unwrap_or_default())
        }
        Zone::Body => super::view::with_laid(ctx, state.place, state.message, |laid| {
            if !laid.done && selection.anchor.max(selection.head).seg >= laid.count() {
                return None;
            }
            Some(range(laid, selection).map(|(lo, hi)| copied(laid, lo, hi)).unwrap_or_default())
        })
        .unwrap_or_else(|| Some(String::new())),
    }
}

/// Copy what is selected in a message to the clipboard.
fn copy(app: &mut MailApp, p: &Value) -> Result<Value, String> {
    let place = place_of(app, p)?;
    let nothing = || tl!("Select some text first").to_string();
    let ctx = app.ctx.clone().ok_or_else(nothing)?;
    let state = app.text_selection.state(place).ok_or_else(nothing)?;
    let Some(text) = selected_text(&ctx, state) else {
        state.copy_when_laid = true;
        return Ok(json!({"pending": true}));
    };
    if text.is_empty() {
        return Err(nothing());
    }
    let chars = text.chars().count();
    ctx.copy_text(for_clipboard(&text));
    Ok(json!({"chars": chars}))
}

/// Select the whole body of the message that is read at a place.
fn select_all(app: &mut MailApp, p: &Value) -> Result<Value, String> {
    let place = place_of(app, p)?;
    let frame = app.ctx.as_ref().map(super::images::frame);
    let nothing = || tl!("No message is being read there").to_string();
    let state = app.text_selection.state(place).filter(|s| frame.is_some_and(|frame| s.frame.saturating_add(1) >= frame)).ok_or_else(nothing)?;
    state.selection = Some(Selection::new(Zone::Body, Point::START, Point::END));
    (state.pressed, state.dragging, state.extending) = (None, None, false);
    let message = state.message;
    app.text_selection.focus = Some(place);
    Ok(json!({"message": message}))
}

/// Copy a link's address: the one a message's link really leads to.
fn copy_link(app: &mut MailApp, p: &Value) -> Result<Value, String> {
    let url = p.get("url").and_then(Value::as_str).ok_or_else(|| "missing `url`".to_string())?.trim();
    if url.len() > MAX_LINK || !mailcraft_html::is_safe_url(url) || mailcraft_html::is_embedded(url) {
        return Err(tl!("That is not a link's address").to_string());
    }
    let ctx = app.ctx.clone().ok_or_else(|| tl!("No message is being read there").to_string())?;
    ctx.copy_text(url.to_string());
    Ok(json!({"chars": url.chars().count()}))
}

/// What is selected in the messages on screen, for `ui.inspect`: where, in which of its texts,
/// and how many characters. Never the text itself.
pub(crate) fn inspect(app: &MailApp, ctx: &egui::Context) -> Value {
    let frame = super::images::frame(ctx);
    let selected = app.text_selection.places.iter().filter(|s| s.frame.saturating_add(1) >= frame && s.selected()).map(|state| {
        let chars = selected_text(ctx, state).map_or(0, |text| text.chars().count());
        let window = match state.place {
            Place::Pane => Value::Null,
            Place::Window(id) => json!(id),
            Place::Quote => json!("quote"),
            Place::Preview => json!("preview"),
        };
        let zone = match state.selection.as_ref().map(|s| s.zone) {
            Some(Zone::Header) => "header",
            _ => "body",
        };
        json!({"window": window, "message": state.message, "in": zone, "chars": chars, "keys": app.text_selection.focus == Some(state.place)})
    });
    Value::Array(selected.collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A place as it is after a frame that showed it.
    fn shown(place: Place, viewport: egui::ViewportId, frame: u64, hovered: bool, selected: bool) -> PlaceState {
        let mut state = PlaceState::new(place);
        (state.message, state.frame, state.viewport, state.hovered) = (1, frame, viewport, hovered);
        if selected {
            state.selection = Some(Selection::new(Zone::Body, Point::START, Point::END));
        }
        state
    }

    #[test]
    fn the_keys_are_for_the_message_the_pointer_went_down_in_or_is_over() {
        let main = egui::ViewportId::ROOT;
        let own = egui::ViewportId::from_hash_of("a message window of the system");
        let (pane, inside, outside) = (Place::Pane, Place::Window(1), Place::Window(2));
        // The reading pane, a message window inside the main window with the pointer over it,
        // and one that is a window of the system, at frame 10.
        let places = vec![shown(pane, main, 10, false, false), shown(inside, main, 10, true, true), shown(outside, own, 10, false, true)];
        let mut all = Selections { places, focus: None };
        // Nowhere did the pointer go down: the keys are for what it is over, in each window.
        assert_eq!((all.key_target(main, 10), all.copy_target(main, 10, false)), (Some(inside), Some(inside)));
        // A window of the system shows one message, and its keys are that message's.
        assert_eq!((all.key_target(own, 10), all.copy_target(own, 10, false)), (Some(outside), Some(outside)));
        // It went down in the pane: Ctrl+A is the pane's, and Ctrl+C copies nothing, for
        // nothing is selected there (not what is selected in the window beside it).
        all.focus = Some(pane);
        assert_eq!((all.key_target(main, 11), all.copy_target(main, 11, false)), (Some(pane), None));
        // That does not reach into another window of the system.
        assert_eq!((all.key_target(own, 11), all.copy_target(own, 11, true)), (Some(outside), None));
        assert_eq!(all.copy_target(own, 11, false), Some(outside));
        // It went down somewhere that is no message (the list): what is selected is copied when
        // that is in one place, and not when a label of the window has a selection of its own.
        all.focus = None;
        all.places = vec![shown(pane, main, 10, false, true), shown(inside, main, 10, false, false)];
        assert_eq!((all.key_target(main, 10), all.copy_target(main, 10, false), all.copy_target(main, 10, true)), (None, Some(pane), None));
        // In two places, with the pointer over neither, there is no telling which is meant.
        all.places = vec![shown(pane, main, 10, false, true), shown(inside, main, 10, false, true)];
        assert_eq!(all.copy_target(main, 10, false), None);
        // What was not shown lately is nowhere: a pane that shows no message has no keys.
        all.focus = Some(pane);
        assert_eq!((all.key_target(main, 12), all.copy_target(main, 12, false)), (None, None));
        assert!(all.any());
        // Only so many places are kept, the ones shown longest ago making room.
        for id in 0..(MAX_PLACES as u64 * 3) {
            let place = Place::Window(100 + id);
            assert!(all.open(place).is_some());
            if let Some(state) = all.state(place) {
                state.frame = 20 + id;
            }
        }
        assert_eq!(all.places.len(), MAX_PLACES);
        assert!(all.state(pane).is_none() && all.state(Place::Window(100 + MAX_PLACES as u64 * 3 - 1)).is_some());
    }
}
