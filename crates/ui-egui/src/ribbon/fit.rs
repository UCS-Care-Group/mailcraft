// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! How the classic ribbon fits a window (#709): each group has four stages, and the row gives
//! them up in an order set by hand. Pure: widths come in, a plan goes out, so that the tests run
//! it at every width without a window.

use super::{Group, Item, MORE_W, Size, Tab};
use crate::widgets::{big_width, small_width};

/// What the classic ribbon sets after a group, past the group's own width: the space either side
/// of the divider, and the spacing round the group.
pub(crate) const GROUP_TAIL: f32 = 13.0;
/// The room before the tab's note.
pub(crate) const NOTE_GAP: f32 = 8.0;
/// The spacing between a group's buttons.
pub(crate) const CELL_GAP: f32 = 1.0;
/// The spacing before `…`.
const MORE_GAP: f32 = 2.0;

/// How much of a group the window shows, from most to least.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Stage {
    /// As classic Outlook lays it out: large buttons and small ones.
    Full,
    /// Every button small, with its label, three to a column.
    Small,
    /// Every button small, with no label: its tooltip names it.
    Icons,
    /// One large button with the group's name; it opens the group as in `Full`.
    Folded,
}

const STAGES: [Stage; 4] = [Stage::Full, Stage::Small, Stage::Icons, Stage::Folded];

/// A piece of a group's layout.
#[derive(Clone, Debug)]
pub(crate) enum Cell<'a> {
    Big(&'a Item),
    /// Up to three small buttons, one over another.
    Column(Vec<&'a Item>),
}

/// The pieces of a group at a stage (`Folded` is laid out as `Full`: it is what the popup shows).
pub(crate) fn cells<'a>(items: &[&'a Item], stage: Stage) -> Vec<Cell<'a>> {
    let mut out = Vec::new();
    let mut column: Vec<&'a Item> = Vec::new();
    let flush = |column: &mut Vec<&'a Item>, out: &mut Vec<Cell<'a>>| {
        if !column.is_empty() {
            out.push(Cell::Column(std::mem::take(column)));
        }
    };
    for item in items {
        if item.size == Size::Small || matches!(stage, Stage::Small | Stage::Icons) {
            column.push(item);
            if column.len() == 3 {
                flush(&mut column, &mut out);
            }
        } else {
            flush(&mut column, &mut out);
            out.push(Cell::Big(item));
        }
    }
    flush(&mut column, &mut out);
    out
}

/// A control that opens a menu and nothing else: it has a `▾`.
pub(crate) fn drops(item: &Item) -> bool {
    item.command.is_none() && item.menu.is_some()
}

/// The controls of a group the classic layout draws.
pub(crate) fn shown(group: &Group) -> Vec<&Item> {
    group.items.iter().filter(|item| item.size != Size::Hidden).collect()
}

/// How wide each stage of a group is, with the divider after it: `text(string, size)` measures a
/// piece of text.
pub(crate) fn group_widths(group: &Group, text: &dyn Fn(&str, f32) -> f32) -> [f32; 4] {
    let items = shown(group);
    let title = tl!(group.title);
    let title_w = text(title, 11.0);
    let mut out = [0.0; 4];
    for (slot, stage) in out.iter_mut().zip(STAGES) {
        if stage == Stage::Folded {
            *slot = big_width(text(title, 11.5), true) + GROUP_TAIL;
            continue;
        }
        let cells = cells(&items, stage);
        let mut content = 0.0;
        for cell in &cells {
            content += match cell {
                Cell::Big(item) => big_width(tl!(item.label).split('\n').map(|l| text(l, 11.5)).fold(0.0, f32::max), drops(item)),
                Cell::Column(items) => {
                    items.iter().map(|item| small_width((stage != Stage::Icons).then(|| text(&item.name(), 11.5)), drops(item))).fold(0.0, f32::max)
                }
            };
        }
        content += CELL_GAP * cells.len().saturating_sub(1) as f32;
        *slot = content.max(title_w + 10.0) + GROUP_TAIL;
    }
    out
}

/// What the row shows at a width.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Fit {
    /// Each group's stage (a group behind `…` is `Folded`, and listed in `behind`).
    pub stages: Vec<Stage>,
    /// The groups behind `…`, in the row's order.
    pub behind: Vec<usize>,
    /// The tab's note has room.
    pub note: bool,
}

/// Fit groups into `available`. `widths` says how wide each is at each stage; `priority` how long
/// it keeps its form, the higher the longer; `note` is the width of the tab's note (0 for none).
///
/// The order: the note; then stage by stage across the row, every group to Small before any
/// loses its labels, every one to Icons before any folds; then, if all are folded and the row is
/// still too wide, groups go behind `…`. Within a step the lowest priority goes first, and of
/// two alike the one further right. A step that would make a group wider is left out, and so is a fold or an icons-only step that gains nothing.
pub(crate) fn fit(widths: &[[f32; 4]], priority: &[u8], note: f32, available: f32) -> Fit {
    let n = widths.len();
    let at = |i: usize, s: Stage| widths.get(i).and_then(|w| w.get(s as usize)).copied().unwrap_or(0.0);
    let mut stages = vec![Stage::Full; n];
    let mut behind = vec![false; n];
    let mut note_on = note > 0.0;
    let total = |stages: &[Stage], behind: &[bool], note_on: bool| {
        let mut sum: f32 =
            (0..n).filter(|i| !behind.get(*i).copied().unwrap_or(false)).map(|i| at(i, stages.get(i).copied().unwrap_or(Stage::Full))).sum();
        if note_on {
            sum += NOTE_GAP + note;
        }
        if behind.iter().any(|b| *b) {
            sum += MORE_GAP + MORE_W;
        }
        sum
    };
    if total(&stages, &behind, note_on) > available {
        note_on = false;
    }
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by_key(|i| (priority.get(*i).copied().unwrap_or(5), std::cmp::Reverse(*i)));
    'steps: for stage in [Stage::Small, Stage::Icons, Stage::Folded] {
        for &i in &order {
            if total(&stages, &behind, note_on) <= available {
                break 'steps;
            }
            let Some(cur) = stages.get(i).copied() else { continue };
            let narrower = if stage == Stage::Small { at(i, stage) <= at(i, cur) } else { at(i, stage) < at(i, cur) };
            if narrower && let Some(s) = stages.get_mut(i) {
                *s = stage;
            }
        }
    }
    for &i in &order {
        if total(&stages, &behind, note_on) <= available {
            break;
        }
        if let Some(b) = behind.get_mut(i) {
            *b = true;
        }
    }
    // What is behind `…` counts as folded, whatever it was before.
    for (s, b) in stages.iter_mut().zip(&behind) {
        if *b {
            *s = Stage::Folded;
        }
    }
    Fit { stages, behind: behind.iter().enumerate().filter(|(_, b)| **b).map(|(i, _)| i).collect(), note: note_on }
}

/// How long each group of a tab keeps its form as the window narrows (higher: longer), by hand,
/// the groups that are used most highest. `which` names the tab (`Home` is told apart by module).
fn priorities(which: &str) -> &'static [(&'static str, u8)] {
    match which {
        "Home mail" => &[("New", 9), ("Delete", 9), ("Respond", 9), ("Move", 7), ("Tags", 6), ("Find", 2), ("Send/Receive", 1)],
        "Home calendar" => {
            &[("New", 9), ("Arrange", 8), ("Go To", 7), ("Respond", 7), ("Appointment", 6), ("Manage Calendars", 4), ("Share", 3), ("Find", 2)]
        }
        "Home people" => &[("New", 9), ("Delete", 8), ("Communicate", 7), ("Actions", 5), ("Share", 3)],
        "Home tasks" => &[("New", 9), ("Delete", 8), ("Manage Task", 7), ("Follow Up", 6), ("Tags", 5), ("Current View", 3)],
        "Message" => &[
            ("Send", 9),
            ("Basic Text", 8),
            ("Include", 7),
            ("Clipboard", 7),
            ("Tags", 5),
            ("Format", 4),
            ("Names", 4),
            ("Editing", 3),
            ("Draft", 3),
        ],
        "Insert" => &[("Include", 9), ("Links", 7), ("Illustrations", 5), ("Text", 5), ("Tables", 4), ("Symbols", 4)],
        "Review" => &[("Proofing", 9), ("Accessibility", 4), ("Language", 3)],
        "Send / Receive" => &[("Send & Receive", 9), ("Accounts", 5), ("Preferences", 4)],
        "Folder" => &[("New", 9), ("Actions", 7), ("Properties", 6), ("Clean Up", 5), ("Favorites", 4), ("Export", 3)],
        "View" => &[("Layout", 9), ("Current View", 8), ("Arrangement", 8), ("Messages", 7), ("Calendar", 6), ("Window", 4), ("Search", 3)],
        _ => &[],
    }
}

/// Set the priority of each group of `tab` (5 for one the table doesn't name).
pub(crate) fn prioritise(tab: &mut Tab, which: &str) {
    let table = priorities(which);
    for group in &mut tab.groups {
        group.priority = table.iter().find(|(title, _)| *title == group.title).map_or(5, |(_, p)| *p);
    }
}
