// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The body of a message being read: parsed and laid out once, painted every frame.
//!
//! A frame costs what is in view: the layout of the last few messages read is kept (with the
//! message it was made from, where it is shown, the width, the theme and the display scale), and
//! painting walks it and paints the pieces inside the clip rectangle. Laying out is spread over
//! frames ([`FRAME_BUDGET`]), so opening a very long message does not stall the window.
//!
//! The look of a message depends on the pane it is read in when its style sheet has `@media`: the
//! blocks are made for the width the body is laid out at and the theme ([`Env`]), kept with the
//! message by what its own `@media` lists say of them ([`Look`]), and made again only when the
//! pane changes in a way one of them notices, never per frame; the last few looks seen stay at
//! hand, so a pane dragged back and forth across a break costs nothing the second time.
//!
//! A message can be on screen in more than one place (the reading pane, a window of its own),
//! each as wide as it likes: every [`Place`] has its own layout, and they share the parsed body.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use egui::{Color32, Rect, Sense, Stroke, Ui, pos2, vec2};
use mailcraft_html::{Block, BlockKind, Env, Marker};
use mailcraft_model::Message;

use super::Place;
use super::colors::Paper;
use super::images::{self, Picture, Pictures, Remote};
use super::layout::{Glue, ItemKind, Laid, PIECE_CHARS, PlaceholderItem, TextItem, cut};
use super::remote::Access;
use super::select::{self, Action};
use crate::MailApp;
use crate::theme::{Tokens, paint_clipped, regular};

/// About how many characters of text are laid out in one frame.
pub const FRAME_BUDGET: usize = 60_000;

/// How many messages' layouts are kept for the reading pane.
const KEPT: usize = 4;

/// How many layouts are kept for windows: one each, and a few that outlive theirs for a moment.
const KEPT_IN_WINDOWS: usize = 16;

/// How many other looks of a message's blocks are kept besides the one in use.
const KEPT_CLASSES: usize = 3;

/// The width a message is laid out at is a multiple of this many points.
const WIDTH_STEP: f32 = 4.0;

/// A message's body as display blocks.
#[derive(Debug, Default)]
pub struct Doc {
    pub blocks: Vec<Block>,
    /// Pictures that are not part of the message: they are placeholders unless the owner asks
    /// for them (and then only those in `remote`).
    pub remote_pictures: usize,
    /// The addresses of the pictures on the web that may be asked for: each once, in reading
    /// order, as the message wrote them, [`super::remote::MAX_PER_MESSAGE`] at most.
    pub remote: Vec<Arc<str>>,
    /// How many more such addresses the message has than may be asked for.
    pub remote_left_out: usize,
    /// Per block: `Some` when it goes on in the paragraph of the block before, which was cut for
    /// its length, with what was dropped at the cut. Empty when nothing was cut.
    pub cuts: Arc<[Option<Glue>]>,
    /// The message's media lists (`@media` and `<style media>`): what its look can change with.
    pub lists: Arc<[String]>,
    /// It has some, so another pane width or the other theme can make another look of it; else
    /// the blocks are the same in all of them.
    pub responsive: bool,
    /// The window is dark and the message brings its own design for dark windows.
    pub sender_dark: bool,
}

impl Doc {
    /// The message's display blocks: from its HTML when it has some, else from its text.
    pub fn of(message: &Message) -> Doc {
        Doc::of_where(message, false)
    }

    /// The same, where pictures on the web can be downloaded when the owner asks
    /// (`can_download`). A message that is nothing but such pictures (a newsletter made of
    /// slices of one picture, with a line of text beside it for programs that show none) is
    /// then shown as what it is, stand-ins first and the offer to download them above: its text
    /// part would say less, and offer nothing.
    pub fn of_where(message: &Message, can_download: bool) -> Doc {
        Doc::of_in(message, can_download, Env::default())
    }

    /// The same for a body of a width and a theme (`env`): the message's `@media` is tested
    /// against it.
    pub fn of_in(message: &Message, can_download: bool, env: Env) -> Doc {
        let lists: Arc<[String]> = message.body.html.as_deref().map(mailcraft_html::media_lists).unwrap_or_default().into();
        let responsive = !lists.is_empty();
        let parsed = message.body.html.as_deref().map(|html| mailcraft_html::parse_in(html, &env));
        let (blocks, sender_dark) = match parsed {
            // HTML that came to nothing (only scripts, only pictures that can't be shown) falls back to the text.
            Some(read) if mailcraft_html::has_text(&read.blocks) || shows_a_picture(&read.blocks, message) || message.body.text.trim().is_empty() => {
                (read.blocks, read.dark_design)
            }
            Some(read) if can_download => {
                let doc = Doc { lists: lists.clone(), responsive, sender_dark: read.dark_design, ..Doc::from_blocks(read.blocks) };
                if !doc.remote.is_empty() {
                    return doc;
                }
                (mailcraft_html::text_blocks(&message.body.text), false)
            }
            _ => (mailcraft_html::text_blocks(&message.body.text), false),
        };
        Doc { lists, responsive, sender_dark, ..Doc::from_blocks(blocks) }
    }

    /// The blocks as a body, with what is known of their pictures on the web.
    pub(super) fn from_blocks(blocks: Vec<Block>) -> Doc {
        let mut doc = Doc::default();
        let mut seen = HashSet::new();
        for picture in mailcraft_html::images(&blocks) {
            let BlockKind::Image { src, width, height, .. } = &picture.kind else { continue };
            if mailcraft_html::is_embedded(src) {
                continue;
            }
            doc.remote_pictures += 1;
            // A picture that says it is two pixels or less is there to be counted, not seen (the
            // parser drops those; this is the second look). Nor is anything asked for that is
            // not on the web.
            let tiny = |v: &Option<u16>| v.is_some_and(|v| v <= 2);
            let src = src.trim();
            if tiny(width) || tiny(height) || !images::is_remote(src) || !seen.insert(src) {
                continue;
            }
            if doc.remote.len() < super::remote::MAX_PER_MESSAGE {
                doc.remote.push(Arc::from(src));
            } else {
                doc.remote_left_out += 1;
            }
        }
        let (blocks, cuts) = in_pieces(blocks);
        doc.blocks = blocks;
        doc.cuts = cuts.into();
        doc
    }
}

/// Whether one of the pictures is written into the message or attached to it.
fn shows_a_picture(blocks: &[Block], message: &Message) -> bool {
    mailcraft_html::images(blocks).iter().any(|b| match &b.kind {
        BlockKind::Image { data: Some(_), .. } => true,
        BlockKind::Image { src, .. } => super::images::attachment(message, src).is_some(),
        _ => false,
    })
}

/// A very long paragraph of the message as several blocks, so that it is laid out a part per
/// frame like everything else. (Paragraphs inside tables are cut up when they are laid out.)
/// With them, for each block that goes on where the one before was cut, what was dropped there:
/// nothing at all when no paragraph was that long.
fn in_pieces(blocks: Vec<Block>) -> (Vec<Block>, Vec<Option<Glue>>) {
    if !blocks.iter().any(|b| b.spans.iter().map(|s| s.text.len()).sum::<usize>() > PIECE_CHARS) {
        return (blocks, Vec::new());
    }
    let mut out = Vec::with_capacity(blocks.len());
    let mut cuts = Vec::with_capacity(blocks.len());
    for block in blocks {
        if block.spans.iter().map(|s| s.text.len()).sum::<usize>() <= PIECE_CHARS {
            out.push(block);
            cuts.push(None);
            continue;
        }
        for (n, (spans, glue)) in cut(&block.spans).into_iter().enumerate() {
            let kind = match &block.kind {
                BlockKind::ListItem { depth, .. } if n > 0 => BlockKind::ListItem { marker: Marker::None, depth: *depth },
                kind => kind.clone(),
            };
            out.push(Block {
                kind,
                spans,
                quote: block.quote,
                align: block.align,
                space: if n == 0 { block.space } else { 0 },
                beside: n == 0 && block.beside,
                gap: if n == 0 { block.gap } else { 0 },
                line_height: block.line_height,
                inset: block.inset,
                line: block.line,
            });
            cuts.push((n > 0).then_some(glue));
        }
    }
    (out, cuts)
}

/// What a message's look is kept by: what each of its media lists says for the body's width and
/// the theme. Two widths with the same verdicts give the same blocks, so a pane dragged about
/// makes a new look only where one of the message's own conditions turns.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Look {
    verdicts: Vec<bool>,
    dark: bool,
}

impl Look {
    /// The window is dark.
    #[cfg(test)]
    pub fn dark(&self) -> bool {
        self.dark
    }

    /// What each media list said, for tests.
    #[cfg(test)]
    pub fn verdicts(&self) -> &[bool] {
        &self.verdicts
    }

    fn of(lists: &[String], env: Env) -> Look {
        Look { verdicts: mailcraft_html::media_verdicts(lists, &env), dark: env.dark() }
    }
}

/// What a layout was made for: when any of it changes, it is made again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Key {
    /// In whole points.
    width: u32,
    dark: bool,
    /// A light page ([`on_light_page`]) in a dark window: turned on as the window turns dark,
    /// `dark` (the page's) stays as it was. In the light theme the page is the window's own, and
    /// turning the switch lays nothing out again.
    light: bool,
    /// The display scale and the height of a line of text, as bits: the fonts or the zoom changed.
    scale: u32,
    line: u32,
    /// How many of the message's pictures from the web had come when it was made; `None` while
    /// they are not shown at all.
    remote: Option<usize>,
}

struct Entry {
    /// The message the blocks were made from. Keeping it keeps its address its own: a message
    /// that is edited is a new one.
    message: Arc<Message>,
    place: Place,
    doc: Arc<Doc>,
    /// The body width and theme last looked at, and the look `doc` has for them (a look that
    /// comes out the same in two widths keeps the one `doc` for both).
    env: Env,
    look: Look,
    /// The blocks made for other looks, most recent first: at most [`KEPT_CLASSES`].
    others: Vec<(Look, Arc<Doc>)>,
    key: Option<Key>,
    laid: Laid,
    /// A layout in the making that takes the place of `laid` once it is done: pictures came, and
    /// what is on screen stays there (and stays scrolled where it is) meanwhile.
    next: Option<(Key, Laid)>,
    used: u64,
    /// Where the body was last painted, for tests that point at things in it.
    #[cfg(test)]
    origin: egui::Pos2,
}

/// The bodies of the messages read lately.
#[derive(Default)]
pub struct Store {
    entries: Vec<Entry>,
    /// How many layouts were begun, for tests.
    #[cfg(test)]
    begun: usize,
    /// How many times a message's blocks were made, for tests.
    #[cfg(test)]
    made: usize,
}

/// Whether two messages show the same body: the same text, HTML and attached bytes.
fn same_body(a: &Message, b: &Message) -> bool {
    a.body == b.body
        && a.attachments.len() == b.attachments.len()
        && a.attachments.iter().zip(&b.attachments).all(|(x, y)| x.content_id == y.content_id && Arc::ptr_eq(&x.data, &y.data))
}

impl Store {
    fn of(ctx: &egui::Context) -> Arc<Mutex<Store>> {
        ctx.data_mut(|d| d.get_temp_mut_or_default::<Arc<Mutex<Store>>>(egui::Id::unique("mailcraft-reading-bodies")).clone())
    }

    /// The entry for `message` shown at `place`, parsing its body when it is not known yet
    /// (`can_download`: for a window that can download pictures, see [`Doc::of_where`]).
    fn entry(&mut self, message: &Arc<Message>, place: Place, frame: u64, can_download: bool, env: Env) -> Option<&mut Entry> {
        let shows = |e: &Entry| {
            // Flagging or filing a message makes a new one of it; its body is still the same.
            Arc::ptr_eq(&e.message, message) || (e.message.id == message.id && same_body(&e.message, message))
        };
        let index = match self.entries.iter().position(|e| e.place == place && shows(e)) {
            Some(index) => index,
            None => {
                // The body may be known from the other place it is shown in.
                let doc =
                    self.entries.iter().find(|e| shows(e) && (!e.doc.responsive || e.look == Look::of(&e.doc.lists, env))).map(|e| e.doc.clone());
                // What was kept here of this message before it changed is of no use any more.
                self.entries.retain(|e| e.place != place || e.message.id != message.id || message.id == 0);
                let (kept, room) = match place {
                    Place::Pane => (self.entries.iter().filter(|e| e.place == Place::Pane).count(), KEPT),
                    Place::Window(_) | Place::Quote | Place::Preview => {
                        // A window shows one message, and a window that is gone shows none.
                        self.entries.retain(|e| e.place == Place::Pane || (e.place != place && e.used.saturating_add(2) >= frame));
                        (self.entries.iter().filter(|e| e.place != Place::Pane).count(), KEPT_IN_WINDOWS)
                    }
                };
                let of_a_kind = |e: &Entry| (e.place == Place::Pane) == (place == Place::Pane);
                if kept >= room
                    && let Some(oldest) = self.entries.iter().enumerate().filter(|(_, e)| of_a_kind(e)).min_by_key(|(_, e)| e.used).map(|(i, _)| i)
                {
                    self.entries.swap_remove(oldest);
                }
                #[cfg(test)]
                {
                    self.made += usize::from(doc.is_none());
                }
                let doc = doc.unwrap_or_else(|| Arc::new(Doc::of_in(message, can_download, env)));
                let look = Look::of(&doc.lists, env);
                self.entries.push(Entry {
                    message: message.clone(),
                    place,
                    doc,
                    env,
                    look,
                    others: Vec::new(),
                    key: None,
                    laid: Laid::default(),
                    next: None,
                    used: frame,
                    #[cfg(test)]
                    origin: egui::Pos2::ZERO,
                });
                self.entries.len() - 1
            }
        };
        let entry = self.entries.get_mut(index)?;
        entry.message = message.clone();
        entry.used = frame;
        Some(entry)
    }
}

/// The message's display blocks, kept from one frame to the next.
#[cfg(test)]
pub fn doc(ctx: &egui::Context, message: &Arc<Message>, place: Place) -> Arc<Doc> {
    doc_where(ctx, message, place, false)
}

/// The same, for a window that can download pictures or not ([`Doc::of_where`]), in a wide pane
/// in the light theme.
#[cfg(test)]
pub fn doc_where(ctx: &egui::Context, message: &Arc<Message>, place: Place, can_download: bool) -> Arc<Doc> {
    doc_in(ctx, message, place, can_download, Env::default())
}

/// A first guess at the width and theme the body will be laid out at in the room `ui` has, for a
/// message whose blocks are made before the body is: the room less the body's margins, and the
/// page's theme (light on a light page, [`on_light_page`]). [`show`] then fits the blocks to the
/// width it really lays out at.
pub fn env_of(app: &MailApp, ui: &Ui, place: Place) -> Env {
    let width = (ui.available_width() - BODY_MARGINS).min(super::MAX_TEXT_WIDTH);
    Env::for_pane(width, Tokens::get(ui.ctx()).dark && !on_light_page(app, place))
}

/// The left and right margins of the body, which the room it is laid out in is less than the pane.
const BODY_MARGINS: f32 = 40.0;

impl Entry {
    /// Make the blocks fit the body width and theme `env`: another look of the message when one
    /// of its media lists says something else for it. Another look is taken from those kept, or
    /// made (and the look in use is kept); one that has the same blocks as the one in use leaves
    /// them, and the layout, as they are. How many times blocks were made.
    fn fit(&mut self, message: &Arc<Message>, can_download: bool, env: Env) -> usize {
        if !self.doc.responsive || self.env == env {
            self.env = env;
            return 0;
        }
        self.env = env;
        let look = Look::of(&self.doc.lists, env);
        if look == self.look {
            return 0;
        }
        let mut made = 0;
        let found = self.others.iter().position(|(l, _)| *l == look).map(|at| self.others.remove(at).1);
        let next = match found {
            Some(doc) => doc,
            None => {
                made = 1;
                let fresh = Arc::new(Doc::of_in(message, can_download, env));
                // A look that has the blocks of the one in use keeps them, and what is laid out
                // and selected in them.
                if fresh.blocks == self.doc.blocks && fresh.sender_dark == self.doc.sender_dark { self.doc.clone() } else { fresh }
            }
        };
        // The layout was made from the blocks that are gone.
        if !Arc::ptr_eq(&next, &self.doc) {
            self.key = None;
            self.next = None;
        }
        let old_look = std::mem::replace(&mut self.look, look);
        let old = std::mem::replace(&mut self.doc, next);
        self.others.insert(0, (old_look, old));
        self.others.truncate(KEPT_CLASSES);
        made
    }
}

/// The message's blocks for the body width and theme `env`, kept from one frame to the next, fitted
/// to it ([`Entry::fit`]).
#[cfg(test)]
pub fn doc_in(ctx: &egui::Context, message: &Arc<Message>, place: Place, can_download: bool, env: Env) -> Arc<Doc> {
    let store = Store::of(ctx);
    let mut store = store.lock().unwrap_or_else(|e| e.into_inner());
    let store = &mut *store;
    let Some(entry) = store.entry(message, place, images::frame(ctx), can_download, env) else {
        return Arc::new(Doc::of_in(message, can_download, env));
    };
    let made = entry.fit(message, can_download, env);
    let doc = entry.doc.clone();
    store.made += made;
    doc
}

/// The message's blocks as they are kept, for what the header says of its pictures: made, when
/// there are none, for a guess at the body's width ([`env_of`]), and not changed to another look
/// here, which [`show`] does once it knows the width it lays out at.
pub fn doc_shown(ctx: &egui::Context, message: &Arc<Message>, place: Place, can_download: bool, env: Env) -> Arc<Doc> {
    let store = Store::of(ctx);
    let mut store = store.lock().unwrap_or_else(|e| e.into_inner());
    match store.entry(message, place, images::frame(ctx), can_download, env) {
        Some(entry) => entry.doc.clone(),
        None => Arc::new(Doc::of_in(message, can_download, env)),
    }
}

/// Paint the body of `message` in the width there is. Links open in the browser and its text can
/// be selected and copied ([`select`]); nothing else in a message is interactive and nothing in
/// it runs. Pictures from the web are shown when `access` says the owner asked for them, as far
/// as they have come.
pub fn show(app: &mut MailApp, ui: &mut Ui, message: &Arc<Message>, place: Place, access: Access) {
    let ctx = ui.ctx().clone();
    let light = on_light_page(app, place);
    let t = if light { Tokens::light() } else { Tokens::get(&ctx) };
    // In steps of a few points, so that dragging the pane's edge does not lay a long message out
    // again for every pixel.
    let width = ((ui.available_width().min(super::MAX_TEXT_WIDTH) / WIDTH_STEP).floor() * WIDTH_STEP).max(1.0);
    // Links whose text names one place and which go to another wear a mark (#508); not in a
    // quote or a preview, which are the owner's own.
    let flagged = if matches!(place, Place::Pane | Place::Window(_)) { super::warnings::mismatched(app, message) } else { Vec::new() };
    let store = Store::of(&ctx);
    let mut store = store.lock().unwrap_or_else(|e| e.into_inner());
    #[cfg(test)]
    let mut begun = 0;
    // The width the body is laid out at, to the pixel, tests the message's `@media`: it is the
    // room this `ui` has (the body's margins are already outside it), in the steps above.
    let env = Env::for_pane(width, t.dark);
    let can_download = super::remote::available(app);
    let Some(made) = store.entry(message, place, images::frame(&ctx), can_download, env).map(|entry| entry.fit(message, can_download, env)) else {
        return;
    };
    #[cfg(test)]
    {
        store.made += made;
    }
    #[cfg(not(test))]
    let _ = made;
    let Some(entry) = store.entry(message, place, images::frame(&ctx), can_download, env) else { return };
    if entry.doc.blocks.is_empty() {
        ui.label(egui::RichText::new(tl!("This message has no text.")).color(t.text_dim));
        if let Some(state) = app.text_selection.state(place) {
            state.no_body();
        }
        return;
    }
    let pictures = Pictures::of(&ctx);
    let mut pictures = pictures.lock().unwrap_or_else(|e| e.into_inner());
    let showing = access == Access::Allowed && !entry.doc.remote.is_empty();
    let key = Key {
        width: width as u32,
        dark: t.dark,
        light: light && Tokens::get(&ctx).dark,
        scale: ctx.pixels_per_point().to_bits(),
        line: ctx.fonts_mut(|f| f.row_height(&regular(super::layout::BASE_SIZE))).to_bits(),
        remote: showing.then(|| entry.doc.remote.iter().filter(|url| matches!(pictures.remote_peek(url), Some(Remote::Ready(_)))).count()),
    };
    if entry.key == Some(key) {
        entry.next = None;
    } else if entry.laid.done && entry.key.is_some_and(|old| Key { remote: key.remote, ..old } == key) {
        // Only the pictures changed: what is shown stays until the new layout is whole.
        if entry.next.as_ref().is_none_or(|(next, _)| *next != key) {
            entry.next = Some((
                key,
                Laid::new(width).showing_remote(showing).on_light_page(light).sender_dark(entry.doc.sender_dark).cut(entry.doc.cuts.clone()),
            ));
            #[cfg(test)]
            {
                begun += 1;
            }
        }
    } else {
        entry.key = Some(key);
        entry.laid = Laid::new(width).showing_remote(showing).on_light_page(light).sender_dark(entry.doc.sender_dark).cut(entry.doc.cuts.clone());
        entry.next = None;
        #[cfg(test)]
        {
            begun += 1;
        }
    }
    let making = match &mut entry.next {
        Some((_, next)) => next,
        None => &mut entry.laid,
    };
    if !making.done {
        making.advance(&ctx, &entry.doc.blocks, Some(&entry.message), &mut pictures, FRAME_BUDGET);
        if !making.done {
            // The rest follows in the next frames.
            ctx.request_repaint();
        }
    }
    if let Some((key, next)) = entry.next.take_if(|(_, next)| next.done) {
        (entry.key, entry.laid) = (Some(key), next);
    }
    drop(pictures);
    let mut state = app.text_selection.state(place);
    // A copy that waited for the layout to be whole is due now.
    let due = state.as_deref_mut().and_then(|state| state.bind(&entry.doc, entry.laid.done));
    let (origin, asked) = paint(ui, &entry.laid, &t, access, state, &flagged);
    #[cfg(test)]
    {
        entry.origin = origin;
        store.begun += begun;
    }
    #[cfg(not(test))]
    let _ = origin;
    // What a menu asked for is a command, run once the layouts are let go of.
    drop(store);
    for action in due.into_iter().chain(asked) {
        select::act(app, place, action);
    }
}

/// Whether a message's body is shown on a light page in the light theme's colours, whatever the
/// window's theme: the owner asked for it, and it is a message being read (not the original
/// under a reply being written, nor File › Signatures' sample).
pub fn on_light_page(app: &MailApp, place: Place) -> bool {
    app.ui.light_messages && matches!(place, Place::Pane | Place::Window(_))
}

/// What the body scrolls on: the light theme's page when [`on_light_page`] (the edges of the
/// scrolling fade to it too), with the light theme's visuals in `ui` for what egui draws on it,
/// its scroll bars; else the window's own.
pub fn page(app: &MailApp, place: Place, ui: &mut Ui) -> egui::Frame {
    if !on_light_page(app, place) {
        return egui::Frame::NONE;
    }
    *ui.visuals_mut() = ui.ctx().style_of(egui::Theme::Light).visuals.clone();
    egui::Frame::NONE.fill(Tokens::light().surface)
}

/// Look at what is laid out of a message's body where it is shown, when it is shown there.
pub(super) fn with_laid<R>(ctx: &egui::Context, place: Place, message: mailcraft_model::Id, read: impl FnOnce(&Laid) -> R) -> Option<R> {
    let store = Store::of(ctx);
    let store = store.lock().unwrap_or_else(|e| e.into_inner());
    store.entries.iter().find(|e| e.place == place && e.message.id == message).map(|e| read(&e.laid))
}

/// What is kept of a message's body where it is shown in the reading pane, for tests: its
/// layout and where it was last painted.
#[cfg(test)]
pub fn kept<R>(ctx: &egui::Context, message: mailcraft_model::Id, read: impl FnOnce(&Laid, egui::Pos2) -> R) -> Option<R> {
    kept_at(ctx, Place::Pane, message, read)
}

/// The same for any place a message is shown in.
#[cfg(test)]
pub fn kept_at<R>(ctx: &egui::Context, place: Place, message: mailcraft_model::Id, read: impl FnOnce(&Laid, egui::Pos2) -> R) -> Option<R> {
    let store = Store::of(ctx);
    let store = store.lock().unwrap();
    store.entries.iter().find(|e| e.place == place && e.message.id == message).map(|e| read(&e.laid, e.origin))
}

/// How many messages' bodies are kept, for tests.
#[cfg(test)]
pub fn kept_count(ctx: &egui::Context) -> usize {
    Store::of(ctx).lock().unwrap().entries.len()
}

/// The look of the blocks in use for a message shown at `place`, for tests.
#[cfg(test)]
pub fn current_look(ctx: &egui::Context, place: Place, message: mailcraft_model::Id) -> Option<Look> {
    let store = Store::of(ctx);
    let store = store.lock().unwrap();
    store.entries.iter().find(|e| e.place == place && e.message.id == message).map(|e| e.look.clone())
}

/// The blocks in use for a message shown at `place`, for tests.
#[cfg(test)]
pub fn current_doc(ctx: &egui::Context, place: Place, message: mailcraft_model::Id) -> Option<Arc<Doc>> {
    let store = Store::of(ctx);
    let store = store.lock().unwrap();
    store.entries.iter().find(|e| e.place == place && e.message.id == message).map(|e| e.doc.clone())
}

/// How many times a message's blocks were made so far, for tests.
#[cfg(test)]
pub fn docs_made(ctx: &egui::Context) -> usize {
    Store::of(ctx).lock().unwrap().made
}

/// How many layouts were begun so far, for tests.
#[cfg(test)]
pub fn layouts_begun(ctx: &egui::Context) -> usize {
    Store::of(ctx).lock().unwrap().begun
}

/// What the pointer is over that leads somewhere, or that has something to say.
struct Hover {
    /// Where a click leads.
    link: Option<Arc<str>>,
    /// What to say besides: where a picture that is not shown would have come from.
    note: Option<String>,
}

/// Paint what is in view of a layout with what is selected in it, and follow the pointer: over
/// its links, and through its text when it selects (`state` is the selection of the place the
/// layout is shown in). Where its top left is, and what its menu was asked to do.
/// `flagged` are the addresses of the links that go somewhere other than their text says: each
/// wears a frame, and its tip says so.
pub fn paint(
    ui: &mut Ui,
    laid: &Laid,
    t: &Tokens,
    access: Access,
    mut state: Option<&mut select::PlaceState>,
    flagged: &[String],
) -> (egui::Pos2, Option<Action>) {
    let (area, _) = ui.allocate_exact_size(vec2(laid.size.x.max(1.0), laid.size.y.max(1.0)), Sense::hover());
    let origin = area.min.to_vec2();
    let clip = ui.clip_rect();
    let ctx = ui.ctx().clone();
    // The pointer is sensed over all of the body that is in view, its margins and the room under
    // a short message too: a drag that selects begins and ends wherever the pointer happens to be.
    let response = ui.interact(clip, ui.scope_id().with("body"), select::sense(ui));
    if let Some(state) = state.as_deref_mut() {
        select::body(ui, &response, laid, area.min, state);
    }
    let selected = state.as_deref().and_then(|state| state.body_range(laid));
    let dragging = state.as_deref().is_some_and(|state| state.dragging_body());
    let marks = selected.map(|_| select::Marks::new(ui, t));
    let painter = ui.painter().clone();
    let pointer = response.hover_pos();
    let mut hover = None;
    let mut on_picture = false;
    let mut pictures = None;
    for item in &laid.items {
        let rect = item.rect.translate(origin);
        if !clip.intersects(rect) {
            continue;
        }
        let over = pointer.filter(|p| rect.contains(*p));
        // The part of it that is selected: of a text its characters, of a picture all or nothing.
        let part = |len: usize| selected.and_then(|(lo, hi)| select::part(lo, hi, item.seg as usize, len));
        match &item.kind {
            ItemKind::Fill { color, radius } => {
                painter.rect_filled(rect, *radius, *color);
            }
            ItemKind::Outline { color, width, radius } => {
                painter.rect_stroke(rect, *radius, Stroke::new(*width, *color), egui::StrokeKind::Inside);
            }
            ItemKind::Dashes { color, dotted } => dashes(&painter, rect, *color, *dotted),
            ItemKind::Bullet { depth, color } => bullet(&painter, rect, *depth, *color),
            ItemKind::Text(text) => {
                let mut galley = ctx.fonts_mut(|f| f.layout_job((*text.job).clone()));
                let at = pos2(rect.min.x + text.anchor, rect.min.y);
                if let Some(link) = over.and_then(|p| link_under(text, &galley, p - at)) {
                    hover = Some(Hover { link: Some(link), note: None });
                }
                if let (Some(marks), Some(part)) = (&marks, part(text.chars as usize)) {
                    marks.text(&mut galley, part);
                }
                if !flagged.is_empty() {
                    for (range, url) in &text.links {
                        if super::warnings::is_mismatched(url, flagged) {
                            for r in char_rects(&galley, range) {
                                painter.rect_stroke(
                                    r.translate(at.to_vec2()).expand(1.5),
                                    2.0,
                                    Stroke::new(1.0, t.orange),
                                    egui::StrokeKind::Outside,
                                );
                            }
                        }
                    }
                }
                painter.galley(at, galley, t.text);
            }
            ItemKind::Picture(picture) => {
                let store = pictures.get_or_insert_with(|| Pictures::of(&ctx));
                let state = store.lock().unwrap_or_else(|e| e.into_inner()).texture(&ctx, &picture.data, rect.size());
                match state {
                    Picture::Ready(texture) => {
                        painter.image(texture.id(), rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                    }
                    Picture::Pending => {
                        painter.rect_stroke(rect, 2.0, Stroke::new(1.0, picture.paper.line(t)), egui::StrokeKind::Inside);
                        ctx.request_repaint();
                    }
                    Picture::Failed => stand_in(&painter, rect, &picture.alt, picture.paper, t, false),
                }
                if let (Some(marks), Some(_)) = (&marks, part(1)) {
                    painter.rect_filled(rect, 0.0, marks.veil);
                }
                on_picture |= over.is_some();
                if over.is_some() && (picture.link.is_some() || !picture.alt.is_empty()) {
                    hover = Some(Hover { link: picture.link.clone(), note: (!picture.alt.is_empty()).then(|| picture.alt.clone()) });
                }
            }
            ItemKind::Placeholder(placeholder) => {
                // What became of it, when it is one that was asked for: a picture that could not
                // be had is marked as such where it would have been, and says why when pointed at.
                let came = (access == Access::Allowed && placeholder.remote).then(|| {
                    let store = pictures.get_or_insert_with(|| Pictures::of(&ctx));
                    store.lock().unwrap_or_else(|e| e.into_inner()).remote_peek(placeholder.src.trim()).cloned()
                });
                let came = came.flatten();
                stand_in(&painter, rect, &placeholder.label, placeholder.paper, t, matches!(came, Some(Remote::Failed(_))));
                if let (Some(marks), Some(_)) = (&marks, part(1)) {
                    painter.rect_filled(rect, 4.0, marks.veil);
                }
                on_picture |= over.is_some();
                if over.is_some() {
                    hover = Some(Hover { link: placeholder.link.clone(), note: Some(placeholder_note(placeholder, access, came.as_ref())) });
                }
            }
        }
    }
    let link =
        hover.as_ref().and_then(|hover| hover.link.clone()).filter(|url| mailcraft_html::is_safe_url(url) && !mailcraft_html::is_embedded(url));
    // The pointer says what it would do: follow a link, or select text (as it does all over the
    // body, the way a page of a word processor has it); over a picture it is the plain arrow.
    if dragging {
        ctx.set_cursor_icon(egui::CursorIcon::Text);
    } else if response.hovered() {
        if link.is_some() {
            ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
        } else if !on_picture {
            ctx.set_cursor_icon(egui::CursorIcon::Text);
        }
    }
    // The menu: Copy and Select All, and for the link it was asked for on, its address.
    let mut asked = None;
    if let Some(state) = state {
        if response.secondary_clicked() {
            state.set_menu_link(link.clone());
        }
        let on_link = state.menu_link();
        response.context_menu(|ui| asked = select::menu(ui, selected.is_some(), true, on_link.as_ref()));
    }
    // While a drag selects, nothing pops up under it; nor over a menu, which opens right where
    // the pointer is.
    if let Some(hover) = hover.filter(|_| !dragging && !egui::Popup::is_any_open(&ctx)) {
        egui::Tooltip::always_open(ctx.clone(), ui.layer_id(), response.id.with("link"), egui::PopupAnchor::Pointer).gap(12.0).show(|ui| {
            ui.set_max_width(520.0);
            if let Some(note) = &hover.note {
                ui.label(clipped(note, TIP_NOTE).as_ref());
            }
            // A link is shown with where it really leads: the host it goes to, then the start of
            // its address, not a tracking address of a thousand characters over the message.
            if let Some(url) = &link {
                if super::warnings::is_mismatched(url, flagged) {
                    ui.label(egui::RichText::new(tl!("This link goes somewhere other than its text says.")).color(t.orange).strong());
                }
                ui.label(link_tip(url));
            }
        });
    }
    // One click follows a link. A drag that began on it selected instead; so do the second and
    // third click of a row, and a click with Shift moves the end of what is selected.
    let followed = response.clicked() && !response.double_clicked() && !response.triple_clicked() && !ui.input(|i| i.modifiers.shift);
    if let Some(url) = link
        && followed
    {
        ctx.open_url(egui::OpenUrl::new_tab(url.to_string()));
    }
    (area.min, asked)
}

/// The most characters of a link's address, and of a picture's description, that a tip shows.
const TIP_ADDRESS: usize = 100;
const TIP_NOTE: usize = 200;

/// `text` cut to `max` characters, with an ellipsis when something was left out.
fn clipped(text: &str, max: usize) -> std::borrow::Cow<'_, str> {
    match text.char_indices().nth(max) {
        Some((at, _)) => format!("{}…", text.get(..at).unwrap_or_default()).into(),
        None => text.into(),
    }
}

/// What a link's tip says of where it leads. For a web address, first the host it goes to as a
/// browser reads it, in ASCII: an international name in its `xn--` form, so that a letter from
/// another script that looks like ours shows, and the host after an `@`, not the name before it
/// (`https://bank.example@evil.example/` goes to evil.example); then the address, with its host
/// written the same way. Anything else (`mailto:`, `tel:`, an address that doesn't read) is the
/// address as it is. Each line at most [`TIP_ADDRESS`] characters.
pub(super) fn link_tip(href: &str) -> String {
    let as_is = || clipped(href, TIP_ADDRESS).into_owned();
    let Some(link) = mailcraft_link::WebLink::parse(href.trim()) else { return as_is() };
    let goes = crate::i18n::fmt(tl!("Goes to {host}"), &[("host", clipped(link.host(), TIP_ADDRESS).as_ref())]);
    format!("{goes}\n{}", clipped(link.as_str(), TIP_ADDRESS))
}

/// What is said of a picture that is not shown, and where it would come from.
fn placeholder_note(placeholder: &PlaceholderItem, access: Access, came: Option<&Remote>) -> String {
    let why = if !placeholder.remote || !images::is_remote(&placeholder.src) {
        tl!("This picture is missing from the message, or can't be shown.").to_string()
    } else {
        match (access, came) {
            (Access::Unavailable, _) => tl!("This picture is on the web. This build can't download pictures.").to_string(),
            (Access::Blocked, _) => tl!("This picture is on the web. It is not downloaded unless you ask.").to_string(),
            (Access::Allowed, Some(Remote::Failed(why))) => crate::i18n::fmt(tl!("This picture couldn't be downloaded: {why}."), &[("why", why)]),
            (Access::Allowed, Some(Remote::Ready(_))) => tl!("This picture was downloaded, but can't be shown.").to_string(),
            (Access::Allowed, None) => tl!("This picture is on its way, or was left out: a message gets only so many.").to_string(),
        }
    };
    let src: String = placeholder.src.chars().take(200).collect();
    format!("{why}\n{src}")
}

/// The character of a laid-out text under a point (from the text's origin), when there is one:
/// not the nearest one, as for a cursor, but the one the point is on.
pub(crate) fn char_at(galley: &egui::Galley, point: egui::Vec2) -> Option<usize> {
    let mut index = 0usize;
    for row in &galley.rows {
        let rect = row.rect();
        if point.y >= rect.min.y && point.y < rect.max.y {
            let x = point.x - rect.min.x;
            return row.row.glyphs.iter().position(|g| x >= g.pos.x && x < g.pos.x + g.advance_width).map(|i| index + i);
        }
        index += row.row.glyphs.len() + usize::from(row.ends_with_newline);
    }
    None
}

/// Where the characters `range` of a galley are, a rectangle for each row they are on (from the
/// galley's origin).
fn char_rects(galley: &egui::Galley, range: &std::ops::Range<usize>) -> Vec<Rect> {
    let mut out = Vec::new();
    let mut index = 0usize;
    for row in &galley.rows {
        let glyphs = &row.row.glyphs;
        let rect = row.rect();
        let mut span: Option<Rect> = None;
        for (i, g) in glyphs.iter().enumerate() {
            if range.contains(&(index + i)) {
                let r = Rect::from_min_max(pos2(rect.min.x + g.pos.x, rect.min.y), pos2(rect.min.x + g.pos.x + g.advance_width, rect.max.y));
                span = Some(span.map_or(r, |s| s.union(r)));
            }
        }
        out.extend(span);
        index += glyphs.len() + usize::from(row.ends_with_newline);
        if index >= range.end || out.len() >= 64 {
            break;
        }
    }
    out
}

/// The link of a laid-out text that a point (from the text's origin) is on.
pub fn link_under(text: &TextItem, galley: &egui::Galley, point: egui::Vec2) -> Option<Arc<str>> {
    let index = char_at(galley, point)?;
    text.links.iter().find(|(range, _)| range.contains(&index)).map(|(_, url)| url.clone())
}

/// A list bullet: a disc, a ring or a square by the depth of the list, as in a browser.
fn bullet(painter: &egui::Painter, rect: Rect, depth: u8, color: Color32) {
    let r = rect.width() / 2.0;
    match depth % 3 {
        0 => {
            painter.circle_filled(rect.center(), r, color);
        }
        1 => {
            painter.circle_stroke(rect.center(), r - 0.5, Stroke::new(1.0, color));
        }
        _ => {
            painter.rect_filled(rect.shrink(0.5), 0.0, color);
        }
    }
}

/// What stands where a picture is not shown: a quiet frame with a small picture mark and the
/// picture's description, when it has one. For one that was asked for and could not be had
/// (`failed`) the mark is struck through, in the colour of a warning.
fn stand_in(painter: &egui::Painter, rect: Rect, label: &str, paper: Paper, t: &Tokens, failed: bool) {
    let ink = Paper { fill: paper.shade(t), own: paper.own }.text(t, true);
    let line = paper.line(t);
    painter.rect(rect, 4.0, paper.shade(t), Stroke::new(1.0, line), egui::StrokeKind::Inside);
    // The mark: a frame, a hill and a sun, drawn here (no artwork is taken from anywhere).
    let mark = Rect::from_center_size(pos2(rect.min.x + 12.0, rect.min.y + 12.0), vec2(12.0, 10.0));
    if rect.width() >= 20.0 && rect.height() >= 20.0 {
        painter.rect_stroke(mark, 1.5, Stroke::new(1.0, ink), egui::StrokeKind::Inside);
        painter.circle_filled(pos2(mark.max.x - 3.5, mark.min.y + 3.5), 1.2, ink);
        let hill = vec![pos2(mark.min.x + 1.5, mark.max.y - 1.5), pos2(mark.min.x + 4.5, mark.min.y + 4.5), pos2(mark.min.x + 8.0, mark.max.y - 1.5)];
        painter.add(egui::Shape::line(hill, Stroke::new(1.0, ink)));
        if failed {
            // Corner to corner and a little past, over a gap in the paper's own colour so that
            // the stroke is told from the frame it crosses.
            let (from, to) = (pos2(mark.min.x - 1.5, mark.max.y + 1.5), pos2(mark.max.x + 1.5, mark.min.y - 1.5));
            painter.line_segment([from, to], Stroke::new(3.5, paper.shade(t)));
            painter.line_segment([from, to], Stroke::new(1.6, t.orange));
        }
    }
    if !label.is_empty() && rect.width() > 40.0 && rect.height() >= 20.0 {
        paint_clipped(painter, pos2(rect.min.x + 23.0, rect.min.y + 12.0), rect.width() - 28.0, label, regular(11.5), ink);
    }
}

/// A dashed or dotted line along the long side of a thin `rect`, centred in it.
fn dashes(painter: &egui::Painter, rect: Rect, color: Color32, dotted: bool) {
    let horizontal = rect.width() >= rect.height();
    let width = if horizontal { rect.height() } else { rect.width() }.max(1.0);
    let (from, to) = if horizontal {
        (pos2(rect.min.x, rect.center().y), pos2(rect.max.x, rect.center().y))
    } else {
        (pos2(rect.center().x, rect.min.y), pos2(rect.center().x, rect.max.y))
    };
    let (dash, gap) = if dotted { (width, width * 1.5) } else { (width * 3.0, width * 2.0) };
    painter.add(egui::Shape::dashed_line(&[from, to], Stroke::new(width, color), dash, gap));
}

#[cfg(test)]
mod tip_tests {
    use super::{TIP_ADDRESS, clipped, link_tip};

    /// The tip's two lines: where the link goes, and its address.
    fn lines(href: &str) -> (String, String) {
        let tip = link_tip(href);
        let (goes, address) = tip.split_once('\n').unwrap();
        (goes.to_string(), address.to_string())
    }

    #[test]
    fn a_link_tip_names_the_host_it_really_goes_to() {
        let (goes, address) = lines("https://lumen.example/agenda?week=3");
        assert!(goes.ends_with(" lumen.example"), "{goes}");
        assert_eq!(address, "https://lumen.example/agenda?week=3");
        // An international name in its ASCII form, so that a look-alike letter shows.
        let (goes, address) = lines("https://b\u{fc}cher.example/kasse");
        assert!(goes.ends_with(" xn--bcher-kva.example"), "{goes}");
        assert_eq!(address, "https://xn--bcher-kva.example/kasse");
        let (goes, _) = lines("https://p\u{0430}ypal.example/");
        assert!(goes.ends_with(" xn--pypal-4ve.example"), "{goes}");
        // What is before an `@` is a name, not where it goes.
        let (goes, address) = lines("https://bank.example@evil.example/login");
        assert!(goes.ends_with(" evil.example") && !goes.contains("bank"), "{goes}");
        assert!(address.starts_with("https://bank.example@evil.example/"), "{address}");
        let (goes, _) = lines("HTTP://Lumen.Example:8080/x");
        assert!(goes.ends_with(" lumen.example"), "{goes}");
        // A long address is cut as before; the host is shown whole.
        let long = format!("https://click.lumen.example/track?u={}", "a1b2".repeat(400));
        let (goes, address) = lines(&long);
        assert!(goes.ends_with(" click.lumen.example"), "{goes}");
        assert_eq!(address.chars().count(), TIP_ADDRESS + 1);
        assert!(address.ends_with('…'));
    }

    #[test]
    fn a_link_tip_shows_other_addresses_as_they_are() {
        assert_eq!(link_tip("mailto:ada@lumen.example?subject=Hi"), "mailto:ada@lumen.example?subject=Hi");
        assert_eq!(link_tip("tel:+15550100"), "tel:+15550100");
        // An address that doesn't read: the start of it, as it is.
        assert_eq!(link_tip("https://[not a host/"), "https://[not a host/");
        let long = format!("http://[{}", "x".repeat(300));
        assert_eq!(link_tip(&long).chars().count(), TIP_ADDRESS + 1);
        assert_eq!(link_tip(""), "");
    }

    #[test]
    fn a_tip_shows_the_start_of_a_long_address_and_all_of_a_short_one() {
        let short = "https://lumen.example/agenda";
        assert_eq!(clipped(short, TIP_ADDRESS), short);
        let long = format!("https://click.lumen.example/track?u={}", "a1b2".repeat(400));
        let shown = clipped(&long, TIP_ADDRESS);
        assert_eq!(shown.chars().count(), TIP_ADDRESS + 1);
        assert!(shown.starts_with("https://click.lumen.example/track?u=") && shown.ends_with('…'));
        // Cut between characters, never inside one.
        let wide = "é".repeat(300);
        assert_eq!(clipped(&wide, TIP_ADDRESS).chars().filter(|c| *c == 'é').count(), TIP_ADDRESS);
        assert_eq!(clipped("", TIP_ADDRESS), "");
        assert_eq!(clipped("abc", 0), "…");
    }
}
