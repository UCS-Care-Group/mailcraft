// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! How many clicks in a row a press is, known as the button goes down, and what a drag from it
//! selects: after two clicks it goes on by words, after three by paragraphs, and what the press
//! took stays selected whichever way it goes, as everywhere else. egui says how many clicks there
//! were only when the button comes up (`double_clicked`), too late for that; they are counted here
//! as egui counts them, by its own limits (`max_double_click_delay`, `max_click_dist`), so that the
//! two agree. The text of a message being read and the body of one being written both select so.

use egui::{Context, Event, Id, PointerButton, Pos2};

/// What a press selects, and what a drag from it grows by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Unit {
    Char,
    Word,
    Paragraph,
}

/// What a press took, `[from, to)`: a place, a word or a paragraph. It stays selected while the
/// drag from it goes on by `unit`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Anchor<T> {
    pub unit: Unit,
    pub from: T,
    pub to: T,
}

impl<T: Ord + Copy> Anchor<T> {
    pub(crate) fn new(unit: Unit, (from, to): (T, T)) -> Anchor<T> {
        Anchor { unit, from, to }
    }

    /// The selection with the pointer over `under`, the `unit` there as `[from, to)`: the end
    /// that stays, then the end that moves.
    pub(crate) fn reach(&self, under: (T, T)) -> (T, T) {
        if under.0 < self.from { (self.to, under.0) } else { (self.from, self.to.max(under.1)) }
    }
}

/// The clicks of a window so far, as egui counts them.
#[derive(Clone, Copy, Default)]
struct Clicks {
    /// The pass they are counted to.
    pass: Option<u64>,
    /// When the last click was, by the window's clock, and where.
    last: Option<(f64, Pos2)>,
    /// When the one before it was.
    before: Option<f64>,
    /// When and where a button last went down, while it is down.
    down: Option<(f64, Pos2)>,
    /// The pointer went too far from there since for its coming up to be a click.
    moved: bool,
    /// What the first button's press in that pass selects.
    pressed: Option<Unit>,
}

impl Clicks {
    /// How many clicks in a row one at `time` and `pos` is: egui's count.
    fn count(&self, time: f64, pos: Pos2, delay: f64, reach: f32) -> u8 {
        let Some((last, at)) = self.last else { return 1 };
        // A third when the two before it were as quick, wherever they were.
        if time - last >= delay || at.distance_sq(pos) >= reach * reach {
            1
        } else if self.before.is_some_and(|before| last - before < delay) {
            3
        } else {
            2
        }
    }
}

/// What the press of the first button that began in this pass of the window selects, by the
/// clicks in a row it is. `None` when it did not go down. A text that selects asks in every pass
/// it is shown, pressed or not, so that the clicks before a press are counted.
pub(crate) fn pressed(ctx: &Context) -> Option<Unit> {
    let id = Id::unique("mailcraft-clicks").with(ctx.viewport_id());
    let pass = ctx.cumulative_pass_nr();
    let mut clicks: Clicks = ctx.data(|d| d.get_temp(id)).unwrap_or_default();
    if clicks.pass != Some(pass) {
        let (delay, reach, longest) = ctx.options(|o| {
            let i = &o.input_options;
            (i.max_double_click_delay, i.max_click_dist, i.max_click_duration)
        });
        ctx.input(|i| {
            clicks.pressed = None;
            // egui's clock moves once a frame: every event of it happens at its time.
            let time = i.time;
            for event in &i.events {
                match event {
                    Event::PointerMoved(pos) => {
                        if let Some((_, from)) = clicks.down {
                            clicks.moved |= from.distance(*pos) > reach;
                        }
                    }
                    Event::PointerButton { pos, button, pressed: true, .. } => {
                        (clicks.down, clicks.moved) = (Some((time, *pos)), false);
                        if *button == PointerButton::Primary {
                            clicks.pressed = Some(match clicks.count(time, *pos, delay, reach) {
                                1 => Unit::Char,
                                2 => Unit::Word,
                                _ => Unit::Paragraph,
                            });
                        }
                    }
                    // A button that comes up near where it went down, soon enough, is a click
                    // (each by itself: a drag can end in the frame of a click); and egui saw one.
                    Event::PointerButton { pos, pressed: false, .. } => {
                        let quick = clicks.down.is_none_or(|(since, _)| time - since <= longest);
                        if !clicks.moved && quick && i.pointer.any_click() {
                            clicks.before = clicks.last.map(|(time, _)| time);
                            clicks.last = Some((time, *pos));
                        }
                        clicks.down = None;
                    }
                    _ => {}
                }
            }
        });
        clicks.pass = Some(pass);
        ctx.data_mut(|d| d.insert_temp(id, clicks));
    }
    clicks.pressed
}
