// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The email-compatibility findings of an HTML signature, as File › Signatures' source editor
//! shows them (`mailcraft_html::compat`).
//!
//! The source is checked once for each exact text it holds, and what was found is kept with that
//! text ([`checked`]): a finding is only ever shown, explained or fixed on the very text it was
//! found in. The checker counts in bytes and egui in characters; [`Checked::of`] converts each
//! range once. The editor underlines each finding through its layouter ([`layout_job`]), errors
//! in the theme's error colour and warnings in its warning colour, only over the finding's own
//! characters; pointing at an underline opens a popup that says what is wrong in the owner's
//! language and, where one exact edit puts it right, offers it ([`popup`]). A left click keeps
//! the popup open. Nothing here keeps a signature: its Save is refused while an error remains
//! ([`save_refusal`]), and the engine refuses it too.

use std::ops::Range;
use std::sync::Arc;

use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, Stroke, Ui};
use mailcraft_html::compat::{CheckError, Fix, Rule, Severity, check};

use crate::MailApp;
use crate::theme::Tokens;

/// The source editor's own id, which its state (caret, undo) is kept under.
pub(crate) fn editor_id() -> egui::Id {
    egui::Id::unique("mailcraft-signature-html-source")
}

/// One finding as the source editor shows it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Mark {
    pub rule: Rule,
    pub severity: Severity,
    /// Its bytes in the checked text.
    pub bytes: Range<usize>,
    /// The same, in characters, as egui counts.
    pub chars: Range<usize>,
    /// The exact edit that puts it right, in the checked text's bytes.
    pub fix: Option<Fix>,
}

/// What the checker found in one exact text.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Checked {
    pub text: String,
    pub marks: Result<Vec<Mark>, CheckError>,
}

impl Checked {
    /// `text` checked now.
    pub(crate) fn of(text: &str) -> Self {
        let marks = check(text).and_then(|found| {
            found
                .into_iter()
                .map(|f| {
                    let chars = chars_of(text, &f.range).ok_or(CheckError::BadRange)?;
                    Ok(Mark { rule: f.rule, severity: f.severity, bytes: f.range, chars, fix: f.fix })
                })
                .collect()
        });
        Self { text: text.to_string(), marks }
    }

    /// The findings, none when the text could not be checked.
    pub(crate) fn marks(&self) -> &[Mark] {
        self.marks.as_deref().unwrap_or(&[])
    }

    fn count(&self, severity: Severity) -> usize {
        self.marks().iter().filter(|m| m.severity == severity).count()
    }

    pub(crate) fn errors(&self) -> usize {
        self.count(Severity::Error)
    }

    pub(crate) fn warnings(&self) -> usize {
        self.count(Severity::Warning)
    }

    /// Whether the signature may not be kept as it is: an error, or a text not known to be clean.
    pub(crate) fn blocks_save(&self) -> bool {
        self.marks.is_err() || self.errors() > 0
    }

    /// `current` with the fix of finding `index` made, only while `current` is still exactly the
    /// text that was checked: an edit found in other words is refused, not guessed at.
    pub(crate) fn fixed(&self, current: &str, index: usize) -> Option<String> {
        if current != self.text {
            return None;
        }
        let fix = self.marks().get(index)?.fix.as_ref()?;
        let head = current.get(..fix.range.start)?;
        let tail = current.get(fix.range.end..)?;
        Some(format!("{head}{}{tail}", fix.replacement))
    }
}

/// The characters of `text` that its bytes `bytes` are, when both ends fall on character
/// boundaries.
pub(crate) fn chars_of(text: &str, bytes: &Range<usize>) -> Option<Range<usize>> {
    let head = text.get(..bytes.start)?;
    let part = text.get(bytes.start..bytes.end)?;
    let from = head.chars().count();
    Some(from..from.checked_add(part.chars().count())?)
}

/// The findings of `text`: those kept for it when it is the text last checked, else checked now
/// and kept in its place.
pub(crate) fn checked(ctx: &egui::Context, text: &str) -> Arc<Checked> {
    let key = egui::Id::unique("mailcraft-signature-compat");
    if let Some(kept) = ctx.data(|d| d.get_temp::<Arc<Checked>>(key)).filter(|c| c.text == text) {
        return kept;
    }
    let fresh = Arc::new(Checked::of(text));
    ctx.data_mut(|d| d.insert_temp(key, fresh.clone()));
    fresh
}

/// How the source editor draws its text: its own font, colour and line height, and the colours
/// it underlines findings in.
pub(crate) struct Look {
    pub font: egui::FontId,
    pub color: Color32,
    pub line_height: f32,
    pub error: Color32,
    pub warning: Color32,
}

impl Look {
    /// The look egui's own code editor gives the field `id` in `ui` (the font, colour and line
    /// height its default layouter uses), with the theme's colours for findings.
    pub(crate) fn of(ui: &Ui, id: egui::Id, t: &Tokens) -> Self {
        let font = egui::TextStyle::Monospace.resolve(ui.style());
        let style: egui::widget_style::TextEditStyle = ui.widget_style(id, &egui::class::Classes::default());
        let color = ui.visuals().override_text_color.unwrap_or(style.atom_layout.text_style.color);
        let line_height = ui.fonts_mut(|f| f.row_height(&font)) + ui.spacing().extra_text_line_spacing;
        Self { font, color, line_height, error: t.red, warning: t.orange }
    }

    fn colour(&self, severity: Severity) -> Color32 {
        match severity {
            Severity::Error => self.error,
            Severity::Warning => self.warning,
        }
    }
}

/// The severities, the more serious first: a severity's place here is its [`rank`].
const BY_RANK: [Severity; 2] = [Severity::Error, Severity::Warning];

/// Where `severity` is in [`BY_RANK`].
fn rank(severity: Severity) -> usize {
    match severity {
        Severity::Error => 0,
        Severity::Warning => 1,
    }
}

/// `text` laid out as the code editor lays it out, with each of `marks` underlined in its
/// severity's colour (the most serious where they overlap) over its own bytes alone. Marks that
/// are not of this text (a range past its end or off a character boundary) underline nothing.
pub(crate) fn layout_job(text: &str, marks: &[Mark], look: &Look, wrap_width: f32) -> LayoutJob {
    let plain = TextFormat { font_id: look.font.clone(), color: look.color, line_height: Some(look.line_height), ..Default::default() };
    let usable: Vec<&Mark> = marks.iter().filter(|m| m.bytes.start < m.bytes.end && text.get(m.bytes.clone()).is_some()).collect();
    let mut cuts: Vec<usize> = vec![0, text.len()];
    for m in &usable {
        cuts.extend([m.bytes.start, m.bytes.end]);
    }
    cuts.sort_unstable();
    cuts.dedup();
    // One sweep over the cuts: a mark is counted from the cut it starts at to the one it ends at,
    // so each part knows how many marks of each severity cover it without looking at them all.
    let mut starts: Vec<(usize, usize)> = usable.iter().map(|m| (m.bytes.start, rank(m.severity))).collect();
    let mut ends: Vec<(usize, usize)> = usable.iter().map(|m| (m.bytes.end, rank(m.severity))).collect();
    starts.sort_unstable();
    ends.sort_unstable();
    let (mut starts, mut ends) = (starts.into_iter().peekable(), ends.into_iter().peekable());
    let mut covering = [0usize; BY_RANK.len()];
    let mut job = LayoutJob::default();
    for pair in cuts.windows(2) {
        let [from, to] = pair else { continue };
        while let Some((_, level)) = starts.next_if(|(at, _)| at <= from) {
            if let Some(n) = covering.get_mut(level) {
                *n = n.saturating_add(1);
            }
        }
        while let Some((_, level)) = ends.next_if(|(at, _)| at <= from) {
            if let Some(n) = covering.get_mut(level) {
                *n = n.saturating_sub(1);
            }
        }
        let Some(part) = text.get(*from..*to) else { continue };
        let worst = BY_RANK.iter().zip(&covering).find(|(_, n)| **n > 0).map(|(severity, _)| *severity);
        let mut format = plain.clone();
        if let Some(severity) = worst {
            format.underline = Stroke::new(1.0, look.colour(severity));
        }
        job.append(part, 0.0, format);
    }
    if job.sections.is_empty() {
        // An empty source still has a row, with the editor's own font.
        job.append("", 0.0, plain);
    }
    job.wrap.max_width = wrap_width;
    // As egui's own editor: trailing white space is kept, since hiding it feels wrong when typing.
    job.keep_trailing_whitespace = true;
    job
}

/// The on-screen rectangles of each of `marks` in `galley` painted at `pos`: for each mark, one
/// rectangle for each row its characters are on. The rows' first characters are counted once,
/// and each mark looks only at the rows and glyphs it is on, so a frame's work is the galley's
/// rows and the marks' own characters, not every glyph again for every mark.
pub(crate) fn mark_rects(galley: &egui::Galley, pos: egui::Pos2, marks: &[Mark]) -> Vec<Vec<egui::Rect>> {
    let mut starts = Vec::with_capacity(galley.rows.len());
    let mut at = 0usize;
    for row in &galley.rows {
        starts.push(at);
        at = at.saturating_add(row.glyphs.len()).saturating_add(usize::from(row.ends_with_newline));
    }
    marks.iter().map(|m| rects(galley, pos, &starts, &m.chars)).collect()
}

/// The rectangles of the characters `chars` of `galley` at `pos`, one for each row they are on;
/// `starts` is where each row's first character is, counting a row's newline as a character.
fn rects(galley: &egui::Galley, pos: egui::Pos2, starts: &[usize], chars: &Range<usize>) -> Vec<egui::Rect> {
    let mut out = Vec::new();
    // The last row that starts at or before the first character: the rows before it end before it.
    let first = starts.partition_point(|start| *start <= chars.start).saturating_sub(1);
    for (row, start) in galley.rows.iter().zip(starts).skip(first) {
        if *start >= chars.end {
            break;
        }
        let from = chars.start.saturating_sub(*start);
        let to = chars.end.saturating_sub(*start).min(row.glyphs.len());
        let Some(glyphs) = row.glyphs.get(from..to) else { continue };
        let mut span: Option<(f32, f32)> = None;
        for glyph in glyphs {
            let x = pos.x + row.pos.x + glyph.pos.x;
            span = Some(span.map_or((x, x + glyph.advance_width), |(l, r)| (l.min(x), r.max(x + glyph.advance_width))));
        }
        if let Some((left, right)) = span {
            let top = pos.y + row.pos.y;
            out.push(egui::Rect::from_min_max(egui::pos2(left, top), egui::pos2(right, top + row.size.y)));
        }
    }
    out
}

/// The popup that is open, for which text and finding.
#[derive(Clone, Debug)]
struct Open {
    text: String,
    index: usize,
    at: egui::Pos2,
    /// Opened by a click: it stays until a press elsewhere. Otherwise it is the hover's
    /// and goes when the pointer leaves the underline and the popup.
    pinned: bool,
    /// Where it was drawn last.
    area: Option<egui::Rect>,
}

/// The interface's words for what a rule means: a title and one sentence. The literals here are
/// what the catalogue extractor sees; the checker has no English of its own.
pub(crate) fn rule_title(rule: Rule) -> &'static str {
    match rule {
        Rule::SvgImage => crate::tl!("SVG picture"),
        Rule::UnsupportedImageFormat => crate::tl!("Picture format that may not show"),
        Rule::CssOnlyImageSize => crate::tl!("Picture sized only by its style"),
        Rule::StylesheetNotSendable => crate::tl!("Style rule that can't be sent"),
        Rule::StyleNotSendable => crate::tl!("Style that can't be sent"),
        Rule::DataUriImage => crate::tl!("Picture written into the HTML"),
        Rule::FormControl => crate::tl!("Form or form field"),
        Rule::EmbeddedContent => crate::tl!("Embedded frame or object"),
        Rule::MediaPlayer => crate::tl!("Video or audio player"),
        Rule::GeneratedContent => crate::tl!("Text added by a style rule"),
        Rule::CssVariable => crate::tl!("CSS variable"),
        Rule::SpaceSeparatedColour => crate::tl!("Colour written with spaces"),
        Rule::Flexbox => crate::tl!("Flexible box layout"),
        Rule::Grid => crate::tl!("Grid layout"),
        Rule::Float => crate::tl!("Floating element"),
        Rule::Positioned => crate::tl!("Positioned element"),
        Rule::MinMaxWidth => crate::tl!("Minimum or maximum width"),
        Rule::RoundedCorners => crate::tl!("Rounded corners"),
        Rule::BoxShadow => crate::tl!("Shadow"),
        Rule::BackgroundImage => crate::tl!("Background picture"),
        Rule::Gradient => crate::tl!("Colour gradient"),
        Rule::RemUnit => crate::tl!("Size in rem"),
        Rule::ViewportUnit => crate::tl!("Size relative to the window"),
        Rule::NoGenericFontFamily => crate::tl!("Font list without a general family"),
    }
}

pub(crate) fn rule_explanation(rule: Rule) -> &'static str {
    match rule {
        Rule::SvgImage => {
            crate::tl!("Classic Outlook for Windows can't show SVG pictures, so this one will be missing there: use a PNG instead.")
        }
        Rule::UnsupportedImageFormat => crate::tl!(
            "Classic Outlook for Windows can't show WebP, AVIF or HEIC pictures, so this one will be missing there: use a PNG or a JPEG instead."
        ),
        Rule::CssOnlyImageSize => crate::tl!(
            "Classic Outlook for Windows ignores a picture's size when only its style sets it, and shows the picture at the file's own size."
        ),
        Rule::StylesheetNotSendable => crate::tl!(
            "This style rule uses something MailCraft can't send safely (such as an import, a web address or an unsupported selector). Remove or simplify the rule."
        ),
        Rule::StyleNotSendable => crate::tl!(
            "This style uses something MailCraft can't send safely (such as a web address for a background picture), so the whole style would be left out, its colours and spacing with it. Remove that part of the style."
        ),
        Rule::DataUriImage => crate::tl!(
            "Classic Outlook for Windows doesn't show a picture whose data is written into the HTML, so this one will be missing there: attach the picture to the signature instead."
        ),
        Rule::FormControl => crate::tl!(
            "Forms, fields and buttons don't work in an email, and Classic Outlook for Windows drops them or shows them as plain text: use a link instead."
        ),
        Rule::EmbeddedContent => crate::tl!(
            "Classic Outlook for Windows, like other mail programs, doesn't show frames or embedded objects, so this part will be missing: link to it instead."
        ),
        Rule::MediaPlayer => crate::tl!(
            "Classic Outlook for Windows can't play video or audio in a message, so recipients see nothing here: link to the recording instead."
        ),
        Rule::GeneratedContent => crate::tl!(
            "Classic Outlook for Windows doesn't show text or symbols that a style rule adds before or after an element, so they will be missing: write them into the HTML itself."
        ),
        Rule::CssVariable => crate::tl!(
            "Classic Outlook for Windows doesn't understand CSS variables, so a style that uses one is lost there: write the value itself instead."
        ),
        Rule::SpaceSeparatedColour => crate::tl!(
            "Classic Outlook for Windows doesn't read a colour whose numbers are separated by spaces, and some mail services drop the whole style because of it: separate them with commas."
        ),
        Rule::Flexbox => crate::tl!(
            "Classic Outlook for Windows ignores flexible box layout, so these items will be stacked or run together there: a table can put them side by side."
        ),
        Rule::Grid => crate::tl!(
            "Classic Outlook for Windows ignores grid layout, so these items will be stacked there: a table with rows and cells lays them out instead."
        ),
        Rule::Float => crate::tl!(
            "Classic Outlook for Windows ignores floating, so this element won't sit beside the others there: use table cells, or an align attribute on a picture or a table."
        ),
        Rule::Positioned => crate::tl!(
            "Classic Outlook for Windows ignores positioning, so this element will be shown in the ordinary flow there: put it in a table cell instead."
        ),
        Rule::MinMaxWidth => crate::tl!(
            "Classic Outlook for Windows ignores a minimum or maximum width here, so this part may be as wide as the whole message there: give a table a fixed width instead."
        ),
        Rule::RoundedCorners => crate::tl!("Classic Outlook for Windows draws square corners, so these corners won't be rounded there."),
        Rule::BoxShadow => crate::tl!("Classic Outlook for Windows doesn't draw shadows, so this one won't appear there."),
        Rule::BackgroundImage => crate::tl!(
            "Classic Outlook for Windows doesn't show background pictures, so only the background colour appears there: make sure the text can be read on it."
        ),
        Rule::Gradient => crate::tl!(
            "Classic Outlook for Windows doesn't draw gradients: give the element a plain background colour as well, for it to show there."
        ),
        Rule::RemUnit => {
            crate::tl!("Classic Outlook for Windows doesn't understand sizes in rem, so this size is ignored there: use pixels instead.")
        }
        Rule::ViewportUnit => crate::tl!(
            "Classic Outlook for Windows doesn't understand sizes relative to the window (vw, vh, vmin, vmax), so this size is ignored there: use pixels or a percentage instead."
        ),
        Rule::NoGenericFontFamily => crate::tl!(
            "Recipients who don't have these fonts get another one, often a serif font in Classic Outlook for Windows: end the list with serif, sans-serif or monospace."
        ),
    }
}

/// What the fix button of a rule says, for a rule whose findings can have one.
pub(crate) fn fix_label(rule: Rule) -> Option<&'static str> {
    match rule {
        Rule::CssOnlyImageSize => Some(crate::tl!("Fix: add the size as width and height attributes")),
        Rule::FormControl | Rule::EmbeddedContent | Rule::MediaPlayer => Some(crate::tl!("Fix: remove this element")),
        Rule::CssVariable => Some(crate::tl!("Fix: put the variable's value in its place")),
        Rule::SpaceSeparatedColour => Some(crate::tl!("Fix: separate the colour's numbers with commas")),
        Rule::SvgImage
        | Rule::UnsupportedImageFormat
        | Rule::StylesheetNotSendable
        | Rule::StyleNotSendable
        | Rule::DataUriImage
        | Rule::GeneratedContent
        | Rule::Flexbox
        | Rule::Grid
        | Rule::Float
        | Rule::Positioned
        | Rule::MinMaxWidth
        | Rule::RoundedCorners
        | Rule::BoxShadow
        | Rule::BackgroundImage
        | Rule::Gradient
        | Rule::RemUnit
        | Rule::ViewportUnit
        | Rule::NoGenericFontFamily => None,
    }
}

/// The line below the editor: how many errors and warnings there are, or that the HTML could
/// not be checked. `None` when there is nothing to say.
pub(crate) fn summary(checked: &Checked) -> Option<String> {
    if checked.marks.is_err() {
        return Some(crate::tl!("This signature's HTML could not be checked for email compatibility.").to_string());
    }
    let mut parts = Vec::new();
    if checked.errors() > 0 {
        parts.push(crate::tln!(checked.errors(), "{n} email compatibility error", "{n} email compatibility errors"));
    }
    if checked.warnings() > 0 {
        parts.push(crate::tln!(checked.warnings(), "{n} warning", "{n} warnings"));
    }
    (!parts.is_empty()).then(|| parts.join(", "))
}

/// Why the signature can't be saved as it is, for Save's hover text; `None` when it can.
pub(crate) fn save_refusal(checked: &Checked) -> Option<String> {
    if checked.marks.is_err() {
        return Some(crate::tl!("This signature's HTML could not be checked for email compatibility, so it can't be saved yet.").to_string());
    }
    let n = checked.errors();
    (n > 0).then(|| {
        crate::tln!(n, "Fix {n} email compatibility error to save this signature.", "Fix {n} email compatibility errors to save this signature.")
    })
}

/// After the source editor `out` was drawn holding `source`: the findings' places in the hit map,
/// the popup of the one pointed at or clicked, and what its fix button asks for. Answers the text
/// with the fix made when the owner pressed it, for the caller to send as typing is sent.
pub(crate) fn popup(app: &mut MailApp, ui: &Ui, t: &Tokens, out: &egui::text_edit::TextEditOutput, source: &str) -> Option<String> {
    let ctx = ui.ctx().clone();
    let checked = checked(&ctx, source);
    let places = mark_rects(&out.galley, out.galley_pos, checked.marks());
    for (i, place) in places.iter().enumerate() {
        if let Some(first) = place.first() {
            app.hit.add("signatureCompatMark", i as u64, *first);
        }
    }
    let key = egui::Id::unique("mailcraft-signature-compat-popup");
    let mut open: Option<Open> = ctx.data(|d| d.get_temp(key)).filter(|o: &Open| o.text == source);
    let pointer = ctx.pointer_hover_pos();
    // Only the editor's own layer: not through a popup or a menu drawn over it.
    let under = pointer.filter(|p| out.response.contains_pointer() && ctx.layer_id_at(*p) == Some(ui.layer_id())).and_then(|p| {
        places.iter().enumerate().find_map(|(i, place)| place.iter().find(|r| r.contains(p)).map(|r| (i, egui::pos2(p.x - 12.0, r.bottom() + 2.0))))
    });
    let near = |o: &Open| {
        pointer.is_some_and(|p| {
            o.area.is_some_and(|a| a.expand(8.0).contains(p))
                || places.get(o.index).is_some_and(|place| place.iter().any(|r| r.expand(6.0).contains(p)))
        })
    };
    let pressed = ctx.input(|i| i.pointer.any_pressed());
    // `Some(next)` replaces what is open; `None` leaves it.
    let next: Option<Option<Open>> = match (under, open.as_ref()) {
        (Some((index, at)), _) if out.response.clicked() => Some(Some(Open { text: source.to_string(), index, at, pinned: true, area: None })),
        (Some((index, at)), current) if current.is_none_or(|o| !o.pinned && o.index != index) => {
            Some(Some(Open { text: source.to_string(), index, at, pinned: false, area: None }))
        }
        (None, Some(o)) if !o.pinned && !near(o) => Some(None),
        (_, Some(o)) if o.pinned && pressed && !near(o) => Some(None),
        _ => None,
    };
    if let Some(next) = next {
        open = next;
    }
    let mut asked = false;
    if let Some(o) = open.as_mut() {
        match checked.marks().get(o.index) {
            Some(mark) => {
                let colour = match mark.severity {
                    Severity::Error => t.red,
                    Severity::Warning => t.orange,
                };
                let shown = egui::Area::new(key.with("area")).order(egui::Order::Foreground).fixed_pos(o.at).constrain(true).show(&ctx, |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.set_max_width(340.0);
                        ui.label(egui::RichText::new(rule_title(mark.rule)).strong().color(colour));
                        ui.label(rule_explanation(mark.rule));
                        match (mark.fix.as_ref(), fix_label(mark.rule)) {
                            (Some(_), Some(label)) => {
                                let button = ui.button(label);
                                app.hit.add("signatureCompatFix", 0, button.rect);
                                button.clicked()
                            }
                            _ => false,
                        }
                    })
                });
                o.area = Some(shown.response.rect);
                app.hit.add("signatureCompatPopup", 0, shown.response.rect);
                asked = shown.inner.inner;
            }
            None => open = None,
        }
    }
    // The fix made only on the text it was found in, which this frame's text is; the text before
    // it is a step of the editor's own undo.
    let mut fixed = None;
    if asked
        && let Some(o) = open.as_ref()
        && let Some(text) = checked.fixed(source, o.index)
    {
        let caret = checked.marks().get(o.index).and_then(|m| m.fix.as_ref()).and_then(|f| chars_of(source, &f.range)).map_or(0, |r| r.start);
        remember_for_undo(&ctx, source, caret);
        fixed = Some(text);
        open = None;
    }
    ctx.data_mut(|d| match open {
        Some(o) => {
            d.insert_temp(key, o);
        }
        None => d.remove::<Open>(key),
    });
    fixed
}

/// `before`, with the caret at `caret`, as a step of the source editor's own undo, so that Ctrl+Z
/// there takes a fix back as it takes back typing: egui's editor remembers only what it saw while
/// it had the keyboard, and a fix is made from the popup.
fn remember_for_undo(ctx: &egui::Context, before: &str, caret: usize) {
    let id = editor_id();
    let mut state = egui::TextEdit::load_state(ctx, id).unwrap_or_default();
    let mut undoer = state.undoer();
    undoer.add_undo(&(egui::text::CCursorRange::one(egui::text::CCursor::new(caret)), before.to_string()));
    state.set_undoer(undoer);
    state.store(ctx, id);
}

#[cfg(test)]
#[path = "compat_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "compat_sweep_tests.rs"]
mod sweep_tests;
