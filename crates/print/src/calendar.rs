//! A calendar printed in the Daily, Work Week, Weekly or Monthly style over a range of days:
//! a page per day, per week or per month, with the hours of the day as a grid (the working
//! hours, widened to whatever happens outside them) or the month as a grid of days.

use mailcraft_time::{Date, DateTime, Weekday, time_short};

use crate::font::{self, FontKind};
use crate::{Document, Item, MAX_PAGES, Page, Paper, Rgb, Text, ink};

/// The most days a calendar print covers.
pub const MAX_DAYS: i64 = 400;
/// The most items read for a print (the rest are left out).
pub const MAX_ITEMS: usize = 20_000;
const MARGIN: f32 = 36.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Style {
    #[default]
    Day,
    WorkWeek,
    Week,
    Month,
}

impl Style {
    pub fn parse(code: &str) -> Option<Style> {
        match code.trim().to_ascii_lowercase().replace([' ', '-', '_'], "").as_str() {
            "day" | "daily" => Some(Style::Day),
            "workweek" => Some(Style::WorkWeek),
            "week" | "weekly" => Some(Style::Week),
            "month" | "monthly" => Some(Style::Month),
            _ => None,
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Style::Day => "day",
            Style::WorkWeek => "workWeek",
            Style::Week => "week",
            Style::Month => "month",
        }
    }
}

/// Something on the calendar, by the clock it is shown in. A whole-day item ends at the
/// midnight after its last day.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub title: String,
    pub location: String,
    pub start: DateTime,
    pub end: DateTime,
    pub all_day: bool,
    pub color: Rgb,
}

impl Default for Entry {
    fn default() -> Self {
        Entry {
            title: String::new(),
            location: String::new(),
            start: DateTime::default(),
            end: DateTime::default(),
            all_day: false,
            color: Rgb(90, 130, 190),
        }
    }
}

#[derive(Clone, Debug)]
pub struct CalendarPrint {
    pub owner: String,
    pub style: Style,
    pub from: Date,
    pub to: Date,
    pub week_start: Weekday,
    pub work_days: Vec<Weekday>,
    pub work_start: u8,
    pub work_end: u8,
    pub twelve_hour: bool,
    pub entries: Vec<Entry>,
}

impl Default for CalendarPrint {
    fn default() -> Self {
        CalendarPrint {
            owner: String::new(),
            style: Style::Day,
            from: Date::default(),
            to: Date::default(),
            week_start: Weekday::Sun,
            work_days: vec![Weekday::Mon, Weekday::Tue, Weekday::Wed, Weekday::Thu, Weekday::Fri],
            work_start: 8,
            work_end: 17,
            twelve_hour: true,
            entries: Vec::new(),
        }
    }
}

fn text(x: f32, y: f32, size: f32, font: FontKind, color: Rgb, s: String) -> Item {
    Item::Text(Text { x, y, size, font, italic: false, color, text: s })
}

/// Whether the entry is (partly) on `day`.
fn on(e: &Entry, day: Date) -> bool {
    if e.all_day {
        let end = if e.end.date > e.start.date { e.end.date } else { e.start.date.add_days(1) };
        return e.start.date <= day && day < end;
    }
    let (from, to) = (DateTime::at_midnight(day), DateTime::at_midnight(day.add_days(1)));
    (e.start < to && e.end > from) || (e.start == e.end && e.start.date == day)
}

/// The entry's minutes on `day`: from and to, within 0–1440.
fn minutes_on(e: &Entry, day: Date) -> (u32, u32) {
    let from = if e.start.date < day { 0 } else { e.start.minutes_of_day() };
    let to = if e.end.date > day { 1440 } else { e.end.minutes_of_day() };
    (from.min(1440), to.clamp(from.min(1440), 1440))
}

fn pale(c: Rgb) -> Rgb {
    let mix = |v: u8| ((u16::from(v) + 3 * 255) / 4) as u8;
    Rgb(mix(c.0), mix(c.1), mix(c.2))
}

fn dark(c: Rgb) -> Rgb {
    let luminance = 0.2126 * f32::from(c.0) + 0.7152 * f32::from(c.1) + 0.0722 * f32::from(c.2);
    if luminance > 170.0 { Rgb(c.0 / 2, c.1 / 2, c.2 / 2) } else { c }
}

/// The calendar's pages.
pub fn layout(c: &CalendarPrint, paper: Paper) -> Document {
    let from = c.from.min(c.to);
    let to = c.to.max(c.from);
    let to = if from.days_until(to) > MAX_DAYS { from.add_days(MAX_DAYS) } else { to };
    let entries: Vec<&Entry> = c.entries.iter().take(MAX_ITEMS).collect();
    let mut pages = Vec::new();
    let mut titles = Vec::new();
    match c.style {
        Style::Day => {
            let mut day = from;
            while day <= to && pages.len() < MAX_PAGES {
                titles.push(day.long());
                pages.push(time_grid(c, &entries, &[day], paper, true));
                day = day.add_days(1);
            }
        }
        Style::WorkWeek | Style::Week => {
            let mut start = from.start_of_week(c.week_start);
            while start <= to && pages.len() < MAX_PAGES {
                let mut days: Vec<Date> = (0..7).map(|i| start.add_days(i)).collect();
                if c.style == Style::WorkWeek {
                    let work: Vec<Weekday> = if c.work_days.is_empty() {
                        vec![Weekday::Mon, Weekday::Tue, Weekday::Wed, Weekday::Thu, Weekday::Fri]
                    } else {
                        c.work_days.clone()
                    };
                    days.retain(|d| work.contains(&d.weekday()));
                }
                if let (Some(first), Some(last)) = (days.first(), days.last()) {
                    titles.push(span_title(*first, *last));
                    pages.push(time_grid(c, &entries, &days, paper, false));
                }
                start = start.add_days(7);
            }
        }
        Style::Month => {
            let mut month = from.start_of_month();
            while month <= to && pages.len() < MAX_PAGES {
                titles.push(month.month_year());
                pages.push(month_grid(c, &entries, month, paper));
                month = month.add_months(1);
            }
        }
    }
    let count = pages.len();
    for (i, (page, title)) in pages.iter_mut().zip(&titles).enumerate() {
        let title = font::fit(FontKind::Bold, title, 18.0, paper.width - 2.0 * MARGIN);
        page.items.insert(0, text(MARGIN, MARGIN + 14.0, 18.0, FontKind::Bold, ink::TEXT, title));
        let owner = font::fit(FontKind::Regular, &c.owner, 8.0, paper.width / 2.0);
        page.items.push(text(MARGIN, paper.height - MARGIN + 18.0, 8.0, FontKind::Regular, ink::DIM, owner));
        let number = format!("{} / {count}", i + 1);
        let w = font::width(FontKind::Regular, &number, 8.0);
        page.items.push(text(paper.width - MARGIN - w, paper.height - MARGIN + 18.0, 8.0, FontKind::Regular, ink::DIM, number));
    }
    let title = titles.first().cloned().unwrap_or_default();
    Document { title, paper, pages, pictures: Vec::new() }
}

/// `October 12 – 16, 2026`, `September 28 – October 4, 2026`, `December 28, 2026 – January 3, 2027`.
fn span_title(first: Date, last: Date) -> String {
    let month = |d: Date| mailcraft_time::month_name(d.month);
    if first == last {
        first.long()
    } else if first.year != last.year {
        format!("{} {}, {} – {} {}, {}", month(first), first.day, first.year, month(last), last.day, last.year)
    } else if first.month != last.month {
        format!("{} {} – {} {}, {}", month(first), first.day, month(last), last.day, last.year)
    } else {
        format!("{} {} – {}, {}", month(first), first.day, last.day, last.year)
    }
}

/// Days side by side with the hours down the page.
fn time_grid(c: &CalendarPrint, entries: &[&Entry], days: &[Date], paper: Paper, one_day: bool) -> Page {
    let mut items = Vec::new();
    let n = days.len().max(1) as f32;
    let left = MARGIN + 44.0;
    let right = paper.width - MARGIN;
    let col = (right - left) / n;
    let mut y = MARGIN + 30.0;
    // Day headings.
    for (i, day) in days.iter().enumerate() {
        let x = left + col * i as f32;
        let label = if one_day { day.weekday().name().to_string() } else { format!("{} {}/{}", day.weekday().short(), day.month, day.day) };
        let label = font::fit(FontKind::Bold, &label, 9.5, col - 6.0);
        items.push(Item::Rect { x, y, w: col, h: 18.0, fill: Some(ink::SHADE), stroke: Some(ink::GRID), width: 0.5 });
        items.push(text(x + 4.0, y + 12.5, 9.5, FontKind::Bold, ink::TEXT, label));
    }
    y += 18.0;
    // Whole-day items, in a strip above the hours.
    let whole: Vec<Vec<&Entry>> = days.iter().map(|d| entries.iter().copied().filter(|e| e.all_day && on(e, *d)).collect()).collect();
    let rows = whole.iter().map(Vec::len).max().unwrap_or(0).min(4);
    if rows > 0 {
        let h = rows as f32 * 13.0 + 4.0;
        for (i, list) in whole.iter().enumerate() {
            let x = left + col * i as f32;
            items.push(Item::Rect { x, y, w: col, h, fill: None, stroke: Some(ink::GRID), width: 0.5 });
            for (j, e) in list.iter().take(rows).enumerate() {
                let more = list.len() > rows && j + 1 == rows;
                let ey = y + 2.0 + j as f32 * 13.0;
                let label = if more { format!("+{} more", list.len() - rows + 1) } else { e.title.clone() };
                if !more {
                    items.push(Item::Rect {
                        x: x + 2.0,
                        y: ey,
                        w: col - 4.0,
                        h: 12.0,
                        fill: Some(pale(e.color)),
                        stroke: Some(dark(e.color)),
                        width: 0.5,
                    });
                }
                items.push(text(x + 5.0, ey + 9.0, 7.5, FontKind::Regular, ink::TEXT, font::fit(FontKind::Regular, &label, 7.5, col - 10.0)));
            }
        }
        y += h;
    }
    // The hours: the working day, widened to what happens before or after it.
    let timed: Vec<Vec<&Entry>> = days.iter().map(|d| entries.iter().copied().filter(|e| !e.all_day && on(e, *d)).collect()).collect();
    let mut first = u32::from(c.work_start.min(23));
    let mut last = u32::from(c.work_end.clamp(1, 24)).max(first + 1);
    for (d, list) in days.iter().zip(&timed) {
        for e in list {
            let (a, b) = minutes_on(e, *d);
            first = first.min(a / 60);
            last = last.max(b.div_ceil(60)).min(24);
        }
    }
    let hours = (last - first).max(1);
    let bottom = paper.height - MARGIN;
    let hour_h = ((bottom - y) / hours as f32).max(4.0);
    for h in 0..=hours {
        let hy = y + h as f32 * hour_h;
        items.push(Item::Line { x1: left - 4.0, y1: hy, x2: right, y2: hy, width: 0.5, color: ink::GRID });
        if h < hours {
            let half = hy + hour_h / 2.0;
            items.push(Item::Line { x1: left, y1: half, x2: right, y2: half, width: 0.3, color: ink::LIGHT });
            let label = time_short(((first + h) % 24) as u8, 0, c.twelve_hour);
            items.push(text(MARGIN, hy + 9.0, 7.5, FontKind::Regular, ink::DIM, font::clean(FontKind::Regular, &label)));
        }
    }
    for i in 0..=days.len() {
        let x = left + col * i as f32;
        items.push(Item::Line { x1: x, y1: y, x2: x, y2: y + hours as f32 * hour_h, width: 0.5, color: ink::GRID });
    }
    // Each day's items, side by side where they overlap.
    let top_min = first * 60;
    for (i, (d, list)) in days.iter().zip(&timed).enumerate() {
        let spans: Vec<(u32, u32, &Entry)> = list
            .iter()
            .map(|e| {
                let (a, b) = minutes_on(e, *d);
                (a, b.max(a + 15), *e)
            })
            .collect();
        let lanes = lanes(&spans);
        for ((a, b, e), (lane, of)) in spans.iter().zip(lanes) {
            let w = col / of as f32;
            let x = left + col * i as f32 + w * lane as f32 + 1.5;
            let ey = y + (a.saturating_sub(top_min)) as f32 / 60.0 * hour_h + 0.5;
            let eh = ((b - a) as f32 / 60.0 * hour_h - 1.0).max(9.0);
            let bw = (w - 3.0).max(2.0);
            items.push(Item::Rect { x, y: ey, w: bw, h: eh, fill: Some(pale(e.color)), stroke: Some(dark(e.color)), width: 0.6 });
            items.push(Item::Rect { x, y: ey, w: 2.5_f32.min(bw), h: eh, fill: Some(dark(e.color)), stroke: None, width: 0.0 });
            let mut lines = vec![(e.title.clone(), FontKind::Bold)];
            let when = format!("{} – {}", e.start.time_short(c.twelve_hour), e.end.time_short(c.twelve_hour));
            lines.push((when, FontKind::Regular));
            if !e.location.trim().is_empty() {
                lines.push((e.location.clone(), FontKind::Regular));
            }
            let size = 7.5;
            let room = ((eh - 2.0) / (size * 1.25)).floor().max(1.0) as usize;
            for (k, (line, f)) in lines.iter().take(room).enumerate() {
                let s = font::fit(*f, line, size, bw - 6.0);
                items.push(text(x + 4.5, ey + 1.0 + size + k as f32 * size * 1.25, size, *f, ink::TEXT, s));
            }
        }
    }
    Page { items }
}

/// For each span (start, end), its lane and how many lanes the overlapping group it is in has.
fn lanes(spans: &[(u32, u32, &Entry)]) -> Vec<(usize, usize)> {
    let mut order: Vec<usize> = (0..spans.len()).collect();
    order.sort_by_key(|&i| spans.get(i).map(|s| (s.0, s.1)));
    let mut out = vec![(0usize, 1usize); spans.len()];
    let mut group: Vec<usize> = Vec::new();
    let mut lane_ends: Vec<u32> = Vec::new();
    let mut group_end = 0u32;
    let close = |group: &mut Vec<usize>, lane_ends: &mut Vec<u32>, out: &mut Vec<(usize, usize)>| {
        let count = lane_ends.len().max(1);
        for &g in group.iter() {
            if let Some(o) = out.get_mut(g) {
                o.1 = count;
            }
        }
        group.clear();
        lane_ends.clear();
    };
    for i in order {
        let Some(&(a, b, _)) = spans.get(i) else { continue };
        if !group.is_empty() && a >= group_end {
            close(&mut group, &mut lane_ends, &mut out);
        }
        let lane = match lane_ends.iter().position(|end| *end <= a) {
            Some(l) => l,
            None if lane_ends.len() < 12 => {
                lane_ends.push(0);
                lane_ends.len() - 1
            }
            // A dozen side by side at once is as many as fit; the rest share the last lane.
            None => lane_ends.len() - 1,
        };
        if let Some(end) = lane_ends.get_mut(lane) {
            *end = b;
        }
        if let Some(o) = out.get_mut(i) {
            o.0 = lane;
        }
        group.push(i);
        group_end = if group.len() == 1 { b } else { group_end.max(b) };
    }
    close(&mut group, &mut lane_ends, &mut out);
    out
}

/// `10am`, `2:30pm`, or `14:30` on a 24-hour clock: a time in a month's little room.
fn compact_time(t: DateTime, twelve_hour: bool) -> String {
    if !twelve_hour {
        return time_short(t.hour, t.minute, false);
    }
    let h = match t.hour % 12 {
        0 => 12,
        h => h,
    };
    let half = if t.hour < 12 { "am" } else { "pm" };
    if t.minute == 0 { format!("{h}{half}") } else { format!("{h}:{:02}{half}", t.minute) }
}

/// A month as a grid of days, the items of each day listed in it.
fn month_grid(c: &CalendarPrint, entries: &[&Entry], month: Date, paper: Paper) -> Page {
    let mut items = Vec::new();
    let first = month.start_of_week(c.week_start);
    let last = month.end_of_month();
    let weeks = (first.days_until(last) / 7 + 1).clamp(4, 6) as usize;
    let left = MARGIN;
    let right = paper.width - MARGIN;
    let col = (right - left) / 7.0;
    let mut y = MARGIN + 30.0;
    for i in 0..7 {
        let day = first.add_days(i);
        let x = left + col * i as f32;
        items.push(Item::Rect { x, y, w: col, h: 16.0, fill: Some(ink::SHADE), stroke: Some(ink::GRID), width: 0.5 });
        items.push(text(x + 4.0, y + 11.5, 8.5, FontKind::Bold, ink::TEXT, day.weekday().name().to_string()));
    }
    y += 16.0;
    let row_h = (paper.height - MARGIN - y) / weeks as f32;
    let size = 7.0;
    let line_h = size * 1.3;
    for w in 0..weeks {
        for i in 0..7 {
            let day = first.add_days((w * 7 + i) as i64);
            let x = left + col * i as f32;
            let ry = y + row_h * w as f32;
            let inside = day.month == month.month && day.year == month.year;
            items.push(Item::Rect {
                x,
                y: ry,
                w: col,
                h: row_h,
                fill: if inside { None } else { Some(ink::SHADE) },
                stroke: Some(ink::GRID),
                width: 0.5,
            });
            let label = if day.day == 1 { format!("{} {}", mailcraft_time::month_short(day.month), day.day) } else { day.day.to_string() };
            items.push(text(x + 3.0, ry + 10.0, 8.0, FontKind::Bold, if inside { ink::TEXT } else { ink::DIM }, label));
            let mut list: Vec<&Entry> = entries.iter().copied().filter(|e| on(e, day)).collect();
            list.sort_by_key(|e| (!e.all_day, e.start, e.end));
            let room = ((row_h - 14.0) / line_h).floor().max(0.0) as usize;
            let shown = if list.len() > room { room.saturating_sub(1) } else { list.len() };
            for (k, e) in list.iter().take(shown).enumerate() {
                let ly = ry + 13.0 + k as f32 * line_h;
                let label = if e.all_day { e.title.clone() } else { format!("{} {}", compact_time(e.start, c.twelve_hour), e.title) };
                if e.all_day {
                    items.push(Item::Rect { x: x + 2.0, y: ly, w: col - 4.0, h: line_h, fill: Some(pale(e.color)), stroke: None, width: 0.0 });
                } else {
                    items.push(Item::Rect { x: x + 3.0, y: ly + 2.5, w: 3.0, h: 3.0, fill: Some(dark(e.color)), stroke: None, width: 0.0 });
                }
                items.push(text(x + 8.0, ly + size, size, FontKind::Regular, ink::TEXT, font::fit(FontKind::Regular, &label, size, col - 11.0)));
            }
            if list.len() > shown && room > 0 {
                let ly = ry + 13.0 + shown as f32 * line_h;
                items.push(text(x + 8.0, ly + size, size, FontKind::Regular, ink::DIM, format!("+{} more", list.len() - shown)));
            }
        }
    }
    Page { items }
}
