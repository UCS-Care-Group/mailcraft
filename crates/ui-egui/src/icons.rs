//! MailCraft's icon set, drawn in code (original artwork; no external icon assets).
//!
//! Icons are designed on a 20×20 grid in two colours: line work (`c`) and an accent (`a`).

use egui::{Color32, FontId, Painter, Pos2, Rect, Shape, Stroke, pos2, vec2};

struct Pen<'a> {
    p: &'a Painter,
    r: Rect,
    c: Color32,
    a: Color32,
    w: f32,
}

impl Pen<'_> {
    fn s(&self) -> f32 {
        self.r.width().min(self.r.height()) / 20.0
    }
    fn pt(&self, x: f32, y: f32) -> Pos2 {
        let s = self.s();
        let o = self.r.center() - vec2(10.0 * s, 10.0 * s);
        pos2(o.x + x * s, o.y + y * s)
    }
    fn stroke(&self, c: Color32) -> Stroke {
        Stroke::new(self.w * self.s(), c)
    }
    fn line(&self, pts: &[(f32, f32)]) {
        self.line_c(pts, self.c);
    }
    fn line_c(&self, pts: &[(f32, f32)], c: Color32) {
        let v: Vec<Pos2> = pts.iter().map(|&(x, y)| self.pt(x, y)).collect();
        self.p.add(Shape::line(v, self.stroke(c)));
    }
    fn closed(&self, pts: &[(f32, f32)], c: Color32) {
        let v: Vec<Pos2> = pts.iter().map(|&(x, y)| self.pt(x, y)).collect();
        self.p.add(Shape::closed_line(v, self.stroke(c)));
    }
    /// A filled convex polygon.
    fn fill(&self, pts: &[(f32, f32)], c: Color32) {
        let v: Vec<Pos2> = pts.iter().map(|&(x, y)| self.pt(x, y)).collect();
        self.p.add(Shape::convex_polygon(v, c, Stroke::NONE));
    }
    fn rect(&self, x0: f32, y0: f32, x1: f32, y1: f32, c: Color32) {
        self.p.rect_stroke(Rect::from_min_max(self.pt(x0, y0), self.pt(x1, y1)), 1.0, self.stroke(c), egui::StrokeKind::Middle);
    }
    fn frect(&self, x0: f32, y0: f32, x1: f32, y1: f32, c: Color32) {
        self.p.rect_filled(Rect::from_min_max(self.pt(x0, y0), self.pt(x1, y1)), 0.5, c);
    }
    fn circle(&self, x: f32, y: f32, r: f32, c: Color32) {
        self.p.circle_stroke(self.pt(x, y), r * self.s(), self.stroke(c));
    }
    fn fcircle(&self, x: f32, y: f32, r: f32, c: Color32) {
        self.p.circle_filled(self.pt(x, y), r * self.s(), c);
    }
    fn text(&self, x: f32, y: f32, size: f32, t: &str, c: Color32, bold: bool) {
        let font = if bold { crate::theme::semibold(size * self.s()) } else { FontId::proportional(size * self.s()) };
        self.p.text(self.pt(x, y), egui::Align2::CENTER_CENTER, t, font, c);
    }
    /// An arc of a circle from `from` to `to` degrees (0 = east, clockwise).
    fn arc(&self, x: f32, y: f32, r: f32, from: f32, to: f32, c: Color32) {
        let steps = 14;
        let v: Vec<Pos2> = (0..=steps)
            .map(|i| {
                let a = (from + (to - from) * i as f32 / steps as f32).to_radians();
                self.pt(x + r * a.cos(), y + r * a.sin())
            })
            .collect();
        self.p.add(Shape::line(v, self.stroke(c)));
    }
    /// An envelope: a box with its flap.
    fn envelope(&self, x0: f32, y0: f32, x1: f32, y1: f32, c: Color32) {
        self.rect(x0, y0, x1, y1, c);
        let mid = (x0 + x1) / 2.0;
        self.line_c(&[(x0, y0 + 0.4), (mid, y0 + (y1 - y0) * 0.55), (x1, y0 + 0.4)], c);
    }
    /// A folder outline.
    fn folder(&self, c: Color32) {
        self.closed(&[(2.5, 5.0), (8.0, 5.0), (9.5, 7.0), (17.5, 7.0), (17.5, 15.5), (2.5, 15.5)], c);
    }
    /// A tray (inbox, outbox).
    fn tray(&self, c: Color32) {
        self.closed(&[(3.0, 11.0), (5.5, 5.0), (14.5, 5.0), (17.0, 11.0), (17.0, 15.5), (3.0, 15.5)], c);
        self.line_c(&[(3.0, 11.0), (7.0, 11.0), (8.0, 13.0), (12.0, 13.0), (13.0, 11.0), (17.0, 11.0)], c);
    }
    /// A bin.
    fn bin(&self, c: Color32) {
        self.line_c(&[(4.0, 6.0), (16.0, 6.0)], c);
        self.line_c(&[(8.0, 6.0), (8.0, 4.0), (12.0, 4.0), (12.0, 6.0)], c);
        self.line_c(&[(5.5, 6.0), (6.3, 16.5), (13.7, 16.5), (14.5, 6.0)], c);
    }
    /// A calendar page with its binding.
    fn calendar(&self, c: Color32) {
        self.rect(3.0, 4.5, 17.0, 16.5, c);
        self.frect(3.0, 4.5, 17.0, 7.5, self.a);
        self.line_c(&[(6.5, 3.0), (6.5, 5.5)], c);
        self.line_c(&[(13.5, 3.0), (13.5, 5.5)], c);
    }
    /// A head and shoulders.
    fn person(&self, x: f32, y: f32, k: f32, c: Color32) {
        self.circle(x, y - 3.2 * k, 2.6 * k, c);
        self.arc(x, y + 5.2 * k, 5.4 * k, 205.0, 335.0, c);
    }
    /// A looping arrow (sync, repeat).
    fn loop_arrows(&self, c: Color32, heads: Color32) {
        self.arc(10.0, 10.0, 6.0, 200.0, 340.0, c);
        self.arc(10.0, 10.0, 6.0, 20.0, 160.0, c);
        self.fill(&[(15.0, 5.2), (18.0, 8.8), (13.6, 9.2)], heads);
        self.fill(&[(5.0, 14.8), (2.0, 11.2), (6.4, 10.8)], heads);
    }
    fn check(&self, x: f32, y: f32, k: f32, c: Color32) {
        self.line_c(&[(x - 3.5 * k, y), (x - 1.0 * k, y + 2.6 * k), (x + 3.8 * k, y - 3.0 * k)], c);
    }
    fn plus(&self, x: f32, y: f32, k: f32, c: Color32) {
        self.line_c(&[(x - k, y), (x + k, y)], c);
        self.line_c(&[(x, y - k), (x, y + k)], c);
    }
}

/// Paint icon `name` into `r`. `c` is the line colour; the accent comes from the theme.
pub fn paint(p: &Painter, r: Rect, name: &str, c: Color32, accent: Color32) {
    let pen = Pen { p, r, c, a: accent, w: 1.25 };
    let a = accent;
    let red = Color32::from_rgb(0xD1, 0x3B, 0x3B);
    let yellow = Color32::from_rgb(0xD9, 0xA4, 0x0C);
    let green = Color32::from_rgb(0x2E, 0x8B, 0x57);
    let blue = Color32::from_rgb(0x2F, 0x6F, 0xD0);
    let soft = a.linear_multiply(0.18);
    match name {
        // Mail
        "mail" | "email" | "unread" | "markUnread" => {
            pen.envelope(2.5, 5.0, 17.5, 15.0, c);
            if name.ends_with("nread") {
                pen.fcircle(16.5, 5.5, 2.6, a);
            }
        }
        "new" | "newMail" => {
            pen.envelope(2.5, 6.0, 15.5, 16.0, c);
            pen.fcircle(15.5, 5.5, 3.6, a);
            pen.plus(15.5, 5.5, 2.0, Color32::WHITE);
        }
        "read" | "markRead" | "toggleRead" => {
            pen.closed(&[(3.0, 9.0), (10.0, 4.0), (17.0, 9.0), (17.0, 16.5), (3.0, 16.5)], c);
            pen.line(&[(3.0, 9.0), (10.0, 13.5), (17.0, 9.0)]);
        }
        "markAllRead" => {
            pen.closed(&[(3.0, 9.0), (10.0, 4.0), (17.0, 9.0), (17.0, 16.5), (3.0, 16.5)], c);
            pen.check(10.0, 11.5, 1.0, a);
        }
        "reply" => {
            pen.line_c(&[(8.5, 5.0), (3.5, 10.0), (8.5, 15.0)], a);
            pen.line_c(&[(3.8, 10.0), (11.0, 10.0), (14.5, 11.5), (16.5, 15.5)], a);
        }
        "replyAll" => {
            pen.line_c(&[(7.0, 5.0), (2.0, 10.0), (7.0, 15.0)], c);
            pen.line_c(&[(11.0, 5.0), (6.0, 10.0), (11.0, 15.0)], a);
            pen.line_c(&[(6.3, 10.0), (13.0, 10.0), (16.0, 11.5), (17.8, 15.5)], a);
        }
        "forward" => {
            pen.line_c(&[(11.5, 5.0), (16.5, 10.0), (11.5, 15.0)], a);
            pen.line_c(&[(16.2, 10.0), (9.0, 10.0), (5.5, 11.5), (3.5, 15.5)], a);
        }
        "delete" | "deleted" | "discard" | "deletePermanently" | "remove" => {
            pen.bin(if name == "deletePermanently" { red } else { c });
            pen.line(&[(8.4, 9.0), (8.7, 14.0)]);
            pen.line(&[(11.6, 9.0), (11.3, 14.0)]);
        }
        "empty" | "emptyFolder" => {
            pen.bin(c);
            pen.line_c(&[(8.0, 9.5), (12.0, 13.5)], red);
            pen.line_c(&[(12.0, 9.5), (8.0, 13.5)], red);
        }
        "archive" => {
            pen.rect(3.0, 4.0, 17.0, 8.0, c);
            pen.line(&[(4.5, 8.0), (4.5, 16.0), (15.5, 16.0), (15.5, 8.0)]);
            pen.line_c(&[(8.0, 11.0), (12.0, 11.0)], a);
        }
        "junk" | "notJunk" => {
            pen.circle(10.0, 10.0, 6.5, if name == "junk" { red } else { c });
            pen.line_c(&[(5.4, 5.4), (14.6, 14.6)], if name == "junk" { red } else { c });
            if name == "notJunk" {
                pen.check(14.5, 14.5, 0.8, green);
            }
        }
        "move" => {
            pen.folder(c);
            pen.line_c(&[(6.5, 11.3), (13.0, 11.3)], a);
            pen.line_c(&[(10.8, 9.0), (13.2, 11.3), (10.8, 13.6)], a);
        }
        "copy" => {
            pen.rect(3.5, 3.0, 12.5, 14.0, c);
            pen.frect(7.5, 6.5, 16.5, 17.5, soft);
            pen.rect(7.5, 6.5, 16.5, 17.5, a);
        }
        "flag" | "toggleFlag" | "followUp" => {
            pen.line(&[(5.0, 3.0), (5.0, 17.5)]);
            pen.fill(&[(5.0, 4.0), (15.5, 4.0), (15.5, 11.0), (5.0, 11.0)], red);
        }
        "clearFlag" => {
            pen.line(&[(5.0, 3.0), (5.0, 17.5)]);
            pen.closed(&[(5.0, 4.0), (15.5, 4.0), (15.5, 11.0), (5.0, 11.0)], c);
        }
        "complete" | "check" | "accept" => pen.check(10.0, 10.5, 1.5, green),
        "categorize" | "category" | "categories" => {
            pen.frect(3.5, 3.5, 9.0, 9.0, red);
            pen.frect(11.0, 3.5, 16.5, 9.0, yellow);
            pen.frect(3.5, 11.0, 9.0, 16.5, green);
            pen.frect(11.0, 11.0, 16.5, 16.5, blue);
        }
        "pin" => {
            pen.fill(&[(7.0, 3.0), (13.0, 3.0), (12.0, 8.5), (8.0, 8.5)], a);
            pen.line_c(&[(5.5, 9.0), (14.5, 9.0)], a);
            pen.line(&[(10.0, 9.0), (10.0, 17.5)]);
        }
        "rules" | "createRule" => {
            pen.envelope(2.5, 3.5, 11.5, 9.5, c);
            pen.line_c(&[(7.0, 11.5), (7.0, 15.0), (12.5, 15.0)], a);
            pen.line_c(&[(10.5, 13.0), (12.7, 15.0), (10.5, 17.0)], a);
            pen.folder_small(13.0, 11.5, c);
        }
        "attach" | "attachment" => pen.line(&[
            (13.0, 5.0),
            (6.5, 11.5),
            (6.5, 14.0),
            (8.5, 16.0),
            (11.0, 16.0),
            (16.0, 11.0),
            (16.0, 8.5),
            (14.0, 7.0),
            (9.0, 12.0),
            (9.5, 13.3),
        ]),
        "send" => {
            pen.fill(&[(3.0, 4.0), (17.5, 10.0), (3.0, 10.0)], a);
            pen.fill(&[(3.0, 10.0), (17.5, 10.0), (3.0, 16.0)], a.linear_multiply(0.7));
            pen.line_c(&[(3.0, 10.0), (10.0, 10.0)], Color32::WHITE);
        }
        "sent" => {
            pen.closed(&[(3.0, 4.5), (17.0, 10.0), (3.0, 15.5), (5.5, 10.0)], c);
            pen.line(&[(5.5, 10.0), (12.0, 10.0)]);
        }
        "drafts" | "editDraft" | "edit" | "rename" => {
            pen.line(&[(4.0, 16.0), (4.5, 12.5), (13.0, 4.0), (16.0, 7.0), (7.5, 15.5), (4.0, 16.0)]);
            pen.line_c(&[(11.3, 5.7), (14.3, 8.7)], a);
        }
        "inbox" => {
            pen.tray(c);
            pen.line_c(&[(10.0, 2.5), (10.0, 8.5)], a);
            pen.line_c(&[(7.8, 6.5), (10.0, 8.8), (12.2, 6.5)], a);
        }
        "outbox" | "export" => {
            pen.tray(c);
            pen.line_c(&[(10.0, 9.0), (10.0, 3.0)], a);
            pen.line_c(&[(7.8, 5.2), (10.0, 2.9), (12.2, 5.2)], a);
        }
        "import" | "open" => {
            pen.line(&[(3.5, 12.0), (3.5, 16.0), (16.5, 16.0), (16.5, 12.0)]);
            pen.line_c(&[(10.0, 3.0), (10.0, 12.0)], a);
            pen.line_c(&[(6.8, 9.0), (10.0, 12.2), (13.2, 9.0)], a);
        }
        "folder" | "custom" | "select" => pen.folder(c),
        "folderNew" | "newFolder" => {
            pen.folder(c);
            pen.plus(13.0, 11.3, 2.4, a);
        }
        "favorite" | "star" | "starOn" => {
            let pts =
                [(10.0, 2.8), (12.1, 7.6), (17.3, 8.1), (13.4, 11.6), (14.5, 16.7), (10.0, 14.0), (5.5, 16.7), (6.6, 11.6), (2.7, 8.1), (7.9, 7.6)];
            if name == "starOn" {
                for i in 0..5 {
                    let (tip, l, r) = (pts[i * 2], pts[(i * 2 + 9) % 10], pts[(i * 2 + 1) % 10]);
                    pen.fill(&[tip, r, (10.0, 10.2), l], yellow);
                }
            }
            pen.closed(&pts, if name == "starOn" { yellow } else { c });
        }
        // Compose
        "save" | "saveDraft" => {
            pen.closed(&[(3.5, 3.5), (13.5, 3.5), (16.5, 6.5), (16.5, 16.5), (3.5, 16.5)], c);
            pen.rect(6.5, 3.5, 12.0, 7.5, c);
            pen.frect(6.0, 11.0, 14.0, 16.5, soft);
            pen.rect(6.0, 11.0, 14.0, 16.5, a);
        }
        "highImportance" | "importanceHigh" => {
            pen.frect(9.0, 3.0, 11.0, 12.0, red);
            pen.fcircle(10.0, 15.5, 1.5, red);
        }
        "lowImportance" | "importanceLow" => {
            pen.line_c(&[(10.0, 3.5), (10.0, 15.5)], blue);
            pen.line_c(&[(5.5, 11.0), (10.0, 15.8), (14.5, 11.0)], blue);
        }
        "signature" => {
            pen.line_c(&[(3.0, 13.0), (5.0, 8.0), (7.0, 13.0), (9.0, 9.0), (11.0, 13.0), (14.0, 11.0)], a);
            pen.line(&[(3.0, 16.5), (17.0, 16.5)]);
            pen.line(&[(13.5, 8.5), (17.0, 3.5)]);
        }
        "checkNames" => {
            pen.person(8.0, 9.5, 1.0, c);
            pen.check(15.0, 13.5, 0.9, green);
        }
        "close" | "decline" | "cancel" => {
            let k = if name == "decline" { red } else { c };
            pen.line_c(&[(5.0, 5.0), (15.0, 15.0)], k);
            pen.line_c(&[(15.0, 5.0), (5.0, 15.0)], k);
        }
        "tentative" => pen.text(10.0, 10.0, 15.0, "?", c, true),
        // Modules and navigation
        "calendar" | "appointment" => {
            pen.calendar(c);
            for (x, y) in [(6.5, 10.5), (10.0, 10.5), (13.5, 10.5), (6.5, 13.5), (10.0, 13.5)] {
                pen.fcircle(x, y, 0.9, c);
            }
        }
        "newAppointment" | "newEvent" => {
            pen.calendar(c);
            pen.plus(10.0, 12.0, 2.6, a);
        }
        "today" => {
            pen.calendar(c);
            pen.fcircle(10.0, 12.0, 2.3, a);
        }
        "day" | "workWeek" | "week" | "month" => {
            pen.rect(3.0, 4.0, 17.0, 16.0, c);
            pen.line(&[(3.0, 7.0), (17.0, 7.0)]);
            match name {
                "day" => pen.frect(5.0, 9.0, 15.0, 14.0, a),
                "workWeek" => {
                    for i in 0..5 {
                        let x = 4.4 + i as f32 * 2.4;
                        pen.frect(x, 8.5, x + 1.6, 14.5, if i == 2 { a } else { c.linear_multiply(0.45) });
                    }
                }
                "week" => {
                    for i in 0..7 {
                        let x = 4.1 + i as f32 * 1.75;
                        pen.frect(x, 8.5, x + 1.1, 14.5, if i == 3 { a } else { c.linear_multiply(0.45) });
                    }
                }
                _ => {
                    for row in 0..3 {
                        for col in 0..4 {
                            let (x, y) = (5.0 + col as f32 * 3.0, 8.4 + row as f32 * 2.5);
                            pen.frect(x, y, x + 1.6, y + 1.4, if (row, col) == (1, 2) { a } else { c.linear_multiply(0.45) });
                        }
                    }
                }
            }
        }
        "people" | "contact" | "account" | "newContact" => {
            pen.person(10.0, 10.0, 1.15, c);
            if name == "newContact" {
                pen.plus(15.5, 5.5, 2.2, a);
            }
        }
        "meeting" | "newMeeting" | "fromMessage" => {
            pen.person(7.0, 10.5, 0.85, c);
            pen.person(13.5, 10.5, 0.85, a);
        }
        // A calendar page that came by mail: an invitation (the flap of its envelope), its
        // cancellation, or somebody's answer to one.
        "meetingRequest" | "meetingCancelled" | "meetingAccepted" | "meetingTentative" | "meetingDeclined" => {
            pen.calendar(c);
            match name {
                "meetingRequest" => pen.line_c(&[(5.5, 10.0), (10.0, 13.6), (14.5, 10.0)], c),
                "meetingAccepted" => pen.check(10.0, 12.2, 0.95, green),
                "meetingTentative" => pen.text(10.0, 12.2, 9.5, "?", c, true),
                // Called off: struck through.
                "meetingCancelled" => pen.line_c(&[(3.5, 17.0), (16.5, 6.0)], red),
                _ => {
                    pen.line_c(&[(7.3, 9.6), (12.7, 15.0)], red);
                    pen.line_c(&[(12.7, 9.6), (7.3, 15.0)], red);
                }
            }
        }
        "tasks" | "task" | "newTask" => {
            pen.rect(3.5, 3.5, 16.5, 16.5, c);
            pen.check(10.0, 10.3, 1.05, a);
        }
        "next" | "forwardPeriod" => pen.line(&[(7.5, 4.5), (13.0, 10.0), (7.5, 15.5)]),
        "previous" | "back" => pen.line(&[(12.5, 4.5), (7.0, 10.0), (12.5, 15.5)]),
        "reminder" | "bell" => {
            pen.line(&[
                (4.0, 14.0),
                (5.5, 12.0),
                (5.5, 8.5),
                (7.5, 5.5),
                (10.0, 4.8),
                (12.5, 5.5),
                (14.5, 8.5),
                (14.5, 12.0),
                (16.0, 14.0),
                (4.0, 14.0),
            ]);
            pen.line_c(&[(8.5, 16.0), (10.0, 17.0), (11.5, 16.0)], a);
        }
        // A seal: a round badge with two ribbon tails and a tick (signed mail).
        "seal" | "sign" => {
            pen.line_c(&[(7.6, 11.6), (6.2, 17.5), (8.4, 16.3), (9.4, 18.0), (10.0, 12.9)], a);
            pen.line_c(&[(12.4, 11.6), (13.8, 17.5), (11.6, 16.3), (10.6, 18.0), (10.0, 12.9)], a);
            pen.circle(10.0, 8.0, 5.2, c);
            pen.check(10.0, 8.2, 0.7, c);
        }
        // A closed padlock with a key's bit beside it (encrypted mail).
        "encrypt" => {
            pen.rect(4.0, 9.0, 13.0, 16.5, c);
            pen.arc(8.5, 9.0, 2.8, 180.0, 360.0, c);
            pen.fcircle(8.5, 12.7, 1.1, a);
            pen.circle(15.5, 6.0, 2.0, a);
            pen.line_c(&[(15.5, 8.0), (15.5, 13.0)], a);
            pen.line_c(&[(15.5, 11.0), (17.0, 11.0)], a);
        }
        // A triangle with an exclamation mark (a signature that needs a second look).
        "warning" => {
            pen.closed(&[(10.0, 3.0), (17.5, 16.5), (2.5, 16.5)], c);
            pen.line_c(&[(10.0, 7.5), (10.0, 12.0)], a);
            pen.fcircle(10.0, 14.2, 0.9, a);
        }
        "private" | "lock" => {
            pen.rect(5.0, 9.0, 15.0, 16.5, c);
            pen.arc(10.0, 9.0, 3.2, 180.0, 360.0, c);
            pen.fcircle(10.0, 12.7, 1.2, a);
        }
        "recurrence" | "repeat" => pen.loop_arrows(c, c),
        // People's time in rows, and the meeting's own time down across them.
        "scheduling" => {
            pen.frect(8.5, 3.0, 13.0, 17.0, soft);
            pen.line_c(&[(8.5, 3.0), (8.5, 17.0)], a);
            pen.line_c(&[(13.0, 3.0), (13.0, 17.0)], a);
            pen.frect(2.5, 5.0, 7.0, 7.0, c);
            pen.frect(14.5, 9.0, 17.5, 11.0, c);
            pen.frect(3.5, 13.0, 6.0, 15.0, c);
            pen.frect(14.0, 13.0, 16.0, 15.0, c);
        }
        "sync" | "request" | "sendReceive" => pen.loop_arrows(c, a),
        "showAs" => {
            pen.rect(3.5, 4.0, 16.5, 16.0, c);
            pen.frect(3.5, 4.0, 7.0, 16.0, a);
        }
        // View
        "search" | "find" | "findRelated" => {
            pen.circle(8.5, 8.5, 5.0, c);
            pen.line_c(&[(12.3, 12.3), (17.0, 17.0)], a);
        }
        "filter" => pen.closed(&[(3.0, 4.0), (17.0, 4.0), (11.5, 10.5), (11.5, 16.5), (8.5, 14.8), (8.5, 10.5)], c),
        "readingPane" => {
            pen.rect(3.0, 4.0, 17.0, 16.0, c);
            pen.frect(10.5, 4.0, 17.0, 16.0, soft);
            pen.line_c(&[(10.5, 4.0), (10.5, 16.0)], a);
        }
        "folderPane" => {
            pen.rect(3.0, 4.0, 17.0, 16.0, c);
            pen.frect(3.0, 4.0, 7.5, 16.0, soft);
            pen.line_c(&[(7.5, 4.0), (7.5, 16.0)], a);
        }
        "conversations" => {
            pen.envelope(2.5, 3.5, 12.5, 10.0, c);
            pen.envelope(7.5, 10.0, 17.5, 16.5, a);
        }
        // The ribbon: a band under its tabs, with a row of commands in it.
        "ribbon" => {
            pen.rect(3.0, 7.0, 17.0, 15.0, c);
            pen.line(&[(4.0, 4.5), (8.0, 4.5)]);
            pen.line_c(&[(10.0, 4.5), (13.0, 4.5)], c.linear_multiply(0.45));
            for x in [6.5, 10.0, 13.5] {
                pen.frect(x - 1.0, 10.0, x + 1.0, 12.0, a);
            }
        }
        "sort" | "reverseSort" => {
            pen.line(&[(6.0, 4.0), (6.0, 16.0)]);
            pen.line(&[(3.5, 6.5), (6.0, 4.0), (8.5, 6.5)]);
            pen.line_c(&[(14.0, 4.0), (14.0, 16.0)], a);
            pen.line_c(&[(11.5, 13.5), (14.0, 16.0), (16.5, 13.5)], a);
        }
        "previewLines" | "list" => {
            for y in [5.0, 10.0, 15.0] {
                pen.fcircle(4.0, y, 1.0, a);
                pen.line(&[(7.0, y), (17.0, y)]);
            }
        }
        "reset" => {
            pen.arc(10.0, 10.0, 6.0, 30.0, 330.0, c);
            pen.fill(&[(14.0, 4.5), (17.8, 7.5), (13.6, 8.6)], c);
        }
        // Quick access and general
        "undo" => {
            pen.arc(10.5, 11.0, 5.5, 200.0, 20.0 + 360.0, c);
            pen.fill(&[(2.6, 6.0), (7.8, 6.6), (4.6, 10.8)], a);
        }
        "redo" => {
            pen.arc(9.5, 11.0, 5.5, 160.0, 340.0, c);
            pen.fill(&[(17.4, 6.0), (12.2, 6.6), (15.4, 10.8)], a);
        }
        "more" => {
            for x in [5.0, 10.0, 15.0] {
                pen.fcircle(x, 10.0, 1.3, c);
            }
        }
        "dropdown" => pen.fill(&[(4.5, 7.5), (15.5, 7.5), (10.0, 13.5)], c),
        "expanded" => pen.line(&[(5.0, 7.5), (10.0, 12.5), (15.0, 7.5)]),
        "collapsed" => pen.line(&[(7.5, 5.0), (12.5, 10.0), (7.5, 15.0)]),
        "launcher" => {
            pen.line(&[(5.0, 15.0), (15.0, 15.0), (15.0, 5.0)]);
            pen.line(&[(8.0, 8.0), (13.5, 13.5)]);
        }
        "plus" | "add" => pen.plus(10.0, 10.0, 5.0, c),
        "minus" => pen.line(&[(5.0, 10.0), (15.0, 10.0)]),
        "options" | "settings" => {
            pen.circle(10.0, 10.0, 3.0, c);
            for i in 0..8 {
                let ang = (i as f32 * 45.0).to_radians();
                pen.line(&[(10.0 + 5.0 * ang.cos(), 10.0 + 5.0 * ang.sin()), (10.0 + 7.2 * ang.cos(), 10.0 + 7.2 * ang.sin())]);
            }
        }
        "info" | "about" => {
            pen.circle(10.0, 10.0, 7.0, a);
            pen.fcircle(10.0, 6.5, 1.0, c);
            pen.line(&[(10.0, 9.0), (10.0, 14.0)]);
        }
        "help" => {
            pen.circle(10.0, 10.0, 7.0, a);
            pen.text(10.0, 10.2, 11.0, "?", c, true);
        }
        "print" => {
            pen.rect(6.0, 3.0, 14.0, 7.0, c);
            pen.rect(3.5, 7.0, 16.5, 13.5, c);
            pen.frect(6.0, 11.5, 14.0, 17.0, soft);
            pen.rect(6.0, 11.5, 14.0, 17.0, a);
        }
        "theme" => {
            pen.circle(10.0, 10.0, 6.5, c);
            pen.p.add(Shape::convex_polygon(
                (0..=12)
                    .map(|i| (90.0 + i as f32 * 15.0).to_radians())
                    .map(|ang: f32| pen.pt(10.0 + 6.5 * ang.cos(), 10.0 + 6.5 * ang.sin()))
                    .collect(),
                c,
                Stroke::NONE,
            ));
        }
        "phone" => {
            pen.rect(6.0, 3.0, 14.0, 17.0, c);
            pen.line_c(&[(8.5, 14.5), (11.5, 14.5)], a);
        }
        "web" | "link" => {
            pen.circle(10.0, 10.0, 6.5, c);
            pen.line(&[(3.5, 10.0), (16.5, 10.0)]);
            pen.arc(10.0, 10.0, 6.5, -90.0, 90.0, Color32::TRANSPARENT);
            pen.line(&[(10.0, 3.5), (7.0, 10.0), (10.0, 16.5), (13.0, 10.0), (10.0, 3.5)]);
        }
        "address" | "location" => {
            pen.closed(&[(10.0, 17.0), (5.0, 9.5), (6.0, 5.5), (10.0, 3.5), (14.0, 5.5), (15.0, 9.5)], c);
            pen.fcircle(10.0, 8.5, 1.8, a);
        }
        "clock" | "time" => {
            pen.circle(10.0, 10.0, 6.5, c);
            pen.line_c(&[(10.0, 6.0), (10.0, 10.0), (13.0, 12.0)], a);
        }
        // A meeting held online: a camera, its lens to the right.
        "onlineMeeting" | "joinMeeting" => {
            pen.frect(2.5, 6.0, 12.0, 14.0, soft);
            pen.rect(2.5, 6.0, 12.0, 14.0, c);
            pen.fill(&[(12.8, 9.2), (17.5, 6.3), (17.5, 13.7), (12.8, 10.8)], a);
        }
        // Another time for a meeting: a clock, and an arrow on to a later hour.
        "proposeTime" => {
            pen.circle(8.0, 9.0, 5.5, c);
            pen.line(&[(8.0, 5.8), (8.0, 9.0), (10.3, 10.4)]);
            pen.line_c(&[(10.5, 16.0), (17.0, 16.0)], a);
            pen.line_c(&[(14.6, 13.7), (17.2, 16.0), (14.6, 18.3)], a);
        }
        // What the people invited are asked for: one line ticked, one with a question.
        "responseOptions" => {
            pen.check(5.5, 6.5, 0.8, green);
            pen.line(&[(10.0, 6.5), (17.0, 6.5)]);
            pen.text(5.5, 13.5, 9.5, "?", a, true);
            pen.line(&[(10.0, 13.5), (17.0, 13.5)]);
        }
        // The world with its meridians, and the hour a place on it has.
        "timeZones" | "globe" => {
            pen.circle(9.0, 10.0, 6.0, c);
            pen.line(&[(3.0, 10.0), (15.0, 10.0)]);
            pen.line(&[(9.0, 4.0), (6.9, 6.6), (6.3, 10.0), (6.9, 13.4), (9.0, 16.0)]);
            pen.line(&[(9.0, 4.0), (11.1, 6.6), (11.7, 10.0), (11.1, 13.4), (9.0, 16.0)]);
            pen.fcircle(14.6, 14.6, 3.6, soft);
            pen.circle(14.6, 14.6, 3.6, a);
            pen.line_c(&[(14.6, 12.6), (14.6, 14.6), (16.0, 15.4)], a);
        }
        "notes" | "description" => {
            pen.rect(4.0, 3.0, 16.0, 17.0, c);
            for y in [7.0, 10.0, 13.0] {
                pen.line(&[(6.5, y), (13.5, y)]);
            }
        }
        // Account setup
        "work" => {
            pen.rect(3.0, 7.0, 17.0, 16.0, c);
            pen.line(&[(7.5, 7.0), (7.5, 4.5), (12.5, 4.5), (12.5, 7.0)]);
            pen.line_c(&[(3.0, 11.0), (8.5, 11.0)], a);
            pen.line_c(&[(11.5, 11.0), (17.0, 11.0)], a);
            pen.rect(8.5, 10.0, 11.5, 12.5, a);
        }
        "server" => {
            for y in [4.0, 11.0] {
                pen.rect(3.5, y, 16.5, y + 5.0, c);
                pen.fcircle(6.5, y + 2.5, 1.0, a);
                pen.line(&[(10.0, y + 2.5), (14.0, y + 2.5)]);
            }
        }
        "device" => {
            pen.rect(3.0, 4.0, 17.0, 13.0, c);
            pen.line(&[(10.0, 13.0), (10.0, 16.0)]);
            pen.line(&[(6.5, 16.0), (13.5, 16.0)]);
            pen.frect(5.5, 6.5, 14.5, 10.5, soft);
        }
        // Formatting text
        "bold" => pen.text(10.0, 10.5, 15.0, "B", c, true),
        "italic" => {
            pen.line(&[(11.8, 4.0), (8.2, 16.0)]);
            pen.line(&[(9.0, 4.0), (14.6, 4.0)]);
            pen.line(&[(5.4, 16.0), (11.0, 16.0)]);
        }
        "underline" => {
            pen.line(&[(6.0, 3.5), (6.0, 10.0)]);
            pen.arc(10.0, 10.0, 4.0, 0.0, 180.0, c);
            pen.line(&[(14.0, 3.5), (14.0, 10.0)]);
            pen.line_c(&[(4.5, 17.0), (15.5, 17.0)], a);
        }
        "strike" => {
            pen.text(10.0, 10.0, 14.0, "S", c, false);
            pen.line_c(&[(3.5, 10.5), (16.5, 10.5)], a);
        }
        "fontSize" => {
            pen.text(7.5, 11.0, 14.0, "A", c, false);
            pen.text(15.0, 12.5, 9.0, "A", c, false);
        }
        "fontColor" => {
            pen.text(10.0, 8.5, 13.0, "A", c, false);
            pen.frect(3.5, 15.0, 16.5, 17.8, a);
        }
        "highlight" => {
            pen.closed(&[(11.5, 3.0), (16.0, 7.5), (9.0, 14.5), (4.5, 10.0)], c);
            pen.line(&[(4.5, 10.0), (3.0, 13.5), (5.5, 13.5), (6.8, 12.2)]);
            pen.frect(3.0, 15.5, 17.0, 17.8, a);
        }
        "bullets" => {
            for y in [5.0, 10.0, 15.0] {
                pen.fcircle(4.5, y, 1.3, c);
                pen.line(&[(8.0, y), (17.0, y)]);
            }
        }
        "numbering" => {
            for (y, n) in [(5.0, "1"), (10.0, "2"), (15.0, "3")] {
                pen.text(4.5, y, 6.0, n, c, true);
                pen.line(&[(8.0, y), (17.0, y)]);
            }
        }
        "indent" | "outdent" => {
            pen.line(&[(3.0, 4.0), (17.0, 4.0)]);
            pen.line(&[(9.0, 8.0), (17.0, 8.0)]);
            pen.line(&[(9.0, 12.0), (17.0, 12.0)]);
            pen.line(&[(3.0, 16.0), (17.0, 16.0)]);
            if name == "indent" {
                pen.fill(&[(3.0, 7.0), (6.5, 10.0), (3.0, 13.0)], a);
            } else {
                pen.fill(&[(6.5, 7.0), (3.0, 10.0), (6.5, 13.0)], a);
            }
        }
        "alignLeft" | "alignCenter" | "alignRight" => {
            for (i, y) in [4.0f32, 8.0, 12.0, 16.0].into_iter().enumerate() {
                let w = if i % 2 == 0 { 14.0 } else { 9.0 };
                let x0 = match name {
                    "alignLeft" => 3.0,
                    "alignCenter" => 10.0 - w / 2.0,
                    _ => 17.0 - w,
                };
                pen.line(&[(x0, y), (x0 + w, y)]);
            }
        }
        "clearFormatting" => {
            pen.text(8.5, 9.0, 13.0, "A", c, false);
            pen.line_c(&[(12.0, 12.0), (17.0, 17.0)], red);
            pen.line_c(&[(17.0, 12.0), (12.0, 17.0)], red);
        }
        "hyperlink" => {
            pen.arc(7.0, 13.0, 3.2, 90.0, 270.0, c);
            pen.line(&[(7.0, 9.8), (10.5, 9.8)]);
            pen.line(&[(7.0, 16.2), (10.5, 16.2)]);
            pen.arc(13.0, 7.0, 3.2, -90.0, 90.0, c);
            pen.line(&[(13.0, 3.8), (9.5, 3.8)]);
            pen.line(&[(13.0, 10.2), (9.5, 10.2)]);
            pen.line_c(&[(7.5, 12.5), (12.5, 7.5)], a);
        }
        "picture" | "insertPicture" => {
            pen.rect(2.5, 4.0, 17.5, 16.0, c);
            pen.line(&[(4.5, 14.0), (8.5, 9.5), (11.5, 12.5), (13.5, 10.5), (16.0, 14.0)]);
            pen.fcircle(13.5, 7.0, 1.4, a);
        }
        "rule" | "horizontalLine" => {
            pen.line(&[(5.0, 4.5), (15.0, 4.5)]);
            pen.line_c(&[(2.5, 10.0), (17.5, 10.0)], a);
            pen.line(&[(5.0, 15.5), (15.0, 15.5)]);
        }
        "spelling" => {
            pen.text(8.0, 7.0, 8.0, "abc", c, true);
            pen.check(11.0, 13.5, 1.2, a);
        }
        "wordCount" => {
            pen.text(10.0, 7.0, 8.0, "123", c, true);
            pen.line(&[(4.0, 12.5), (16.0, 12.5)]);
            pen.line(&[(4.0, 16.0), (12.0, 16.0)]);
        }
        "symbol" => pen.text(10.0, 10.5, 14.0, "\u{3a9}", c, false),
        "dateTime" => {
            pen.rect(3.0, 4.5, 13.0, 14.5, c);
            pen.line(&[(3.0, 7.5), (13.0, 7.5)]);
            pen.circle(13.5, 13.5, 4.0, a);
            pen.line_c(&[(13.5, 11.5), (13.5, 13.5), (15.0, 14.5)], a);
        }
        "formatHtml" => pen.text(10.0, 10.5, 7.5, "HTML", c, true),
        "fixedWidth" => pen.text(10.0, 10.0, 12.0, "{ }", c, true),
        "formatPlain" => {
            pen.line(&[(4.0, 5.0), (16.0, 5.0)]);
            pen.line(&[(4.0, 10.0), (16.0, 10.0)]);
            pen.line(&[(4.0, 15.0), (11.0, 15.0)]);
        }
        "tick" => pen.check(10.0, 10.5, 1.5, c),
        // Anything else: a lettered tile, so a missing drawing is visible rather than empty.
        _ => {
            pen.rect(3.5, 3.5, 16.5, 16.5, c);
            let letter: String = name.chars().next().map(|ch| ch.to_uppercase().collect()).unwrap_or_default();
            pen.text(10.0, 10.3, 9.5, &letter, c, true);
        }
    }
}

impl Pen<'_> {
    /// A small folder with its top-left corner at (`x`, `y`).
    fn folder_small(&self, x: f32, y: f32, c: Color32) {
        self.closed(&[(x, y), (x + 2.2, y), (x + 3.0, y + 1.2), (x + 5.5, y + 1.2), (x + 5.5, y + 5.5), (x, y + 5.5)], c);
    }
}

/// The names [`paint`] draws (not a lettered tile); tests check ribbon icons against it.
pub const NAMES: &[&str] = &[
    "mail",
    "email",
    "unread",
    "markUnread",
    "new",
    "newMail",
    "read",
    "markRead",
    "toggleRead",
    "markAllRead",
    "reply",
    "replyAll",
    "forward",
    "delete",
    "deleted",
    "discard",
    "deletePermanently",
    "remove",
    "empty",
    "emptyFolder",
    "archive",
    "junk",
    "notJunk",
    "move",
    "copy",
    "flag",
    "toggleFlag",
    "followUp",
    "clearFlag",
    "complete",
    "check",
    "accept",
    "categorize",
    "category",
    "categories",
    "pin",
    "rules",
    "createRule",
    "attach",
    "attachment",
    "send",
    "sent",
    "drafts",
    "editDraft",
    "edit",
    "rename",
    "inbox",
    "outbox",
    "export",
    "import",
    "open",
    "folder",
    "custom",
    "select",
    "folderNew",
    "newFolder",
    "favorite",
    "star",
    "starOn",
    "save",
    "saveDraft",
    "highImportance",
    "importanceHigh",
    "lowImportance",
    "importanceLow",
    "signature",
    "checkNames",
    "close",
    "decline",
    "cancel",
    "tentative",
    "calendar",
    "appointment",
    "newAppointment",
    "newEvent",
    "today",
    "day",
    "workWeek",
    "week",
    "month",
    "people",
    "contact",
    "account",
    "newContact",
    "meeting",
    "newMeeting",
    "fromMessage",
    "meetingRequest",
    "meetingCancelled",
    "meetingAccepted",
    "meetingTentative",
    "meetingDeclined",
    "tasks",
    "task",
    "newTask",
    "next",
    "forwardPeriod",
    "previous",
    "back",
    "reminder",
    "bell",
    "private",
    "lock",
    "seal",
    "sign",
    "encrypt",
    "warning",
    "recurrence",
    "repeat",
    "scheduling",
    "sync",
    "request",
    "sendReceive",
    "showAs",
    "search",
    "find",
    "findRelated",
    "filter",
    "readingPane",
    "folderPane",
    "conversations",
    "ribbon",
    "sort",
    "reverseSort",
    "previewLines",
    "list",
    "reset",
    "undo",
    "redo",
    "more",
    "dropdown",
    "expanded",
    "collapsed",
    "launcher",
    "plus",
    "add",
    "minus",
    "options",
    "settings",
    "info",
    "about",
    "help",
    "print",
    "theme",
    "phone",
    "web",
    "link",
    "address",
    "location",
    "clock",
    "time",
    "onlineMeeting",
    "joinMeeting",
    "proposeTime",
    "responseOptions",
    "timeZones",
    "globe",
    "notes",
    "description",
    "work",
    "server",
    "device",
    "tick",
    "bold",
    "italic",
    "underline",
    "strike",
    "fontSize",
    "fontColor",
    "highlight",
    "bullets",
    "numbering",
    "indent",
    "outdent",
    "alignLeft",
    "alignCenter",
    "alignRight",
    "clearFormatting",
    "hyperlink",
    "picture",
    "insertPicture",
    "rule",
    "horizontalLine",
    "spelling",
    "wordCount",
    "symbol",
    "dateTime",
    "formatHtml",
    "formatPlain",
    "fixedWidth",
];
