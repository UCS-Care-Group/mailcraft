// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Writing in rich text: the document, the selection, the formatting the next character typed
//! takes, and undo inside the message.

use serde::{Deserialize, Deserializer, Serialize};

use super::find::{Place, moved};
use super::{Format, ListKind, MAX_CHARS, RichText, Style};
use crate::ModelError;

/// Most undo steps one message keeps.
pub const MAX_UNDO: usize = 100;

/// Most characters the undo steps of one message keep between them; the oldest go first.
const MAX_UNDO_CHARS: usize = 4 * MAX_CHARS;

/// What is selected: from `anchor` (where the selection started) to `head` (where the caret
/// is). Equal for a caret with nothing selected.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct Selection {
    pub anchor: usize,
    pub head: usize,
}

impl Selection {
    pub fn caret(at: usize) -> Selection {
        Selection { anchor: at, head: at }
    }

    pub fn start(&self) -> usize {
        self.anchor.min(self.head)
    }

    pub fn end(&self) -> usize {
        self.anchor.max(self.head)
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    fn clamp(self, len: usize) -> Selection {
        Selection { anchor: self.anchor.min(len), head: self.head.min(len) }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Step {
    text: RichText,
    selection: Selection,
}

/// The owner's text in a message being written, with its selection and its own undo.
///
/// Every edit is one undo step, except that characters typed one after another join the step of
/// the first. An edit that fails changes nothing. Only the text and the selection are saved;
/// the undo steps live as long as the message is open.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RichBody {
    text: RichText,
    selection: Selection,
    /// The style the next typed character takes, when it was chosen with nothing selected.
    #[serde(skip)]
    pending: Option<Style>,
    #[serde(skip)]
    undo: Vec<Step>,
    #[serde(skip)]
    redo: Vec<Step>,
    /// Where typing goes on in the same undo step.
    #[serde(skip)]
    typing: Option<usize>,
}

/// A body as stored: plain text (what a message being written held before rich text), or the
/// document and its selection.
#[derive(Deserialize)]
#[serde(untagged)]
enum RawBody {
    Plain(String),
    Rich {
        #[serde(default)]
        text: RichText,
        #[serde(default)]
        selection: Selection,
    },
}

impl<'de> Deserialize<'de> for RichBody {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<RichBody, D::Error> {
        Ok(match RawBody::deserialize(d)? {
            RawBody::Plain(text) => RichBody::from_plain(&text),
            RawBody::Rich { text, selection } => {
                let selection = selection.clamp(text.len());
                RichBody { text, selection, ..RichBody::default() }
            }
        })
    }
}

impl RichBody {
    pub fn new(text: RichText) -> RichBody {
        RichBody { text, ..RichBody::default() }
    }

    pub fn from_plain(text: &str) -> RichBody {
        RichBody::new(RichText::from_plain(text))
    }

    pub fn text(&self) -> &RichText {
        &self.text
    }

    /// The text without formatting (positions count its characters).
    pub fn plain(&self) -> String {
        self.text.text()
    }

    pub fn selection(&self) -> Selection {
        self.selection
    }

    /// The formatting chosen for the next typed character, if one was.
    pub fn pending(&self) -> Option<&Style> {
        self.pending.as_ref()
    }

    /// The style the next typed character takes: what was chosen for it, or the style of the
    /// character before the caret (a link doesn't grow by typing at its end).
    pub fn typing_style(&self) -> Style {
        self.pending.clone().unwrap_or_else(|| {
            let mut style = self.text.style_at(self.selection.start());
            style.link = None;
            style
        })
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Run an edit on a copy and keep it when it works, as one undo step (or as part of the last
    /// one, `typing`).
    fn edit(&mut self, typing: bool, f: impl FnOnce(&mut RichText, &mut Selection) -> Result<(), ModelError>) -> Result<(), ModelError> {
        let mut text = self.text.clone();
        let mut selection = self.selection;
        f(&mut text, &mut selection)?;
        let selection = selection.clamp(text.len());
        if text == self.text && selection == self.selection {
            return Ok(());
        }
        if text != self.text && !typing {
            self.push_undo();
        }
        self.text = text;
        self.selection = selection;
        Ok(())
    }

    fn push_undo(&mut self) {
        self.undo.push(Step { text: self.text.clone(), selection: self.selection });
        self.redo.clear();
        let mut kept: usize = self.undo.iter().map(|s| s.text.len()).sum();
        while self.undo.len() > MAX_UNDO || (kept > MAX_UNDO_CHARS && self.undo.len() > 1) {
            let gone = self.undo.remove(0);
            kept = kept.saturating_sub(gone.text.len());
        }
    }

    /// Put the caret or the selection somewhere. The formatting chosen for the next character
    /// is forgotten.
    pub fn select(&mut self, anchor: usize, head: usize) -> Result<(), ModelError> {
        let len = self.text.len();
        if anchor > len || head > len {
            return Err(ModelError::Invalid(format!("the text has {len} characters")));
        }
        self.selection = Selection { anchor, head };
        self.pending = None;
        self.typing = None;
        Ok(())
    }

    /// Type `text` over the selection, in [`RichBody::typing_style`]. A line break starts a new
    /// paragraph of the same kind, except that a line break alone in an empty list item ends the
    /// list there (Enter on an empty bullet), as word processors do.
    pub fn type_text(&mut self, text: &str) -> Result<(), ModelError> {
        let style = self.typing_style();
        let sel = self.selection;
        if sel.is_empty()
            && matches!(text, "\n" | "\r" | "\r\n")
            && self.text.list_at(sel.head) != ListKind::None
            && self.text.paragraph_is_empty(sel.head)
        {
            // A level out first, then out of the list.
            if self.text.level_at(sel.head) > 0 {
                self.edit(false, |t, _| t.indent(sel.head, sel.head, -1).map(|_| ()))?;
            } else {
                self.edit(false, |t, _| t.set_list(sel.head, sel.head, ListKind::None))?;
            }
            self.typing = None;
            return Ok(());
        }
        let continues = sel.is_empty() && self.typing == Some(sel.head) && !text.contains(['\n', '\r']) && self.can_undo();
        let mut end = 0;
        self.edit(continues, |t, s| {
            t.delete(sel.start(), sel.end())?;
            end = t.insert(sel.start(), text, &style)?;
            *s = Selection::caret(end);
            Ok(())
        })?;
        self.pending = None;
        self.typing = (!text.contains(['\n', '\r'])).then_some(end);
        Ok(())
    }

    /// Put a pasted document over the selection.
    pub fn paste(&mut self, fragment: &RichText) -> Result<(), ModelError> {
        let sel = self.selection;
        self.edit(false, |t, s| {
            t.delete(sel.start(), sel.end())?;
            let end = t.insert_rich(sel.start(), fragment)?;
            *s = Selection::caret(end);
            Ok(())
        })?;
        self.pending = None;
        self.typing = None;
        Ok(())
    }

    /// Remove `[from, to)`; the caret goes where it was.
    pub fn delete(&mut self, from: usize, to: usize) -> Result<(), ModelError> {
        self.edit(false, |t, s| {
            t.delete(from, to)?;
            *s = Selection::caret(from.min(to));
            Ok(())
        })?;
        self.typing = None;
        Ok(())
    }

    /// Backspace: the selection, or the character (or paragraph break) before the caret. At the
    /// start of a list item it first makes the item an ordinary paragraph.
    pub fn backspace(&mut self) -> Result<(), ModelError> {
        let sel = self.selection;
        if !sel.is_empty() {
            return self.delete(sel.start(), sel.end());
        }
        let at = sel.head;
        let at_start = at == 0 || self.plain().chars().nth(at - 1) == Some('\n');
        if at_start && self.text.list_at(at) != ListKind::None {
            return self.edit(false, |t, _| t.set_list(at, at, ListKind::None));
        }
        if at == 0 {
            return Ok(());
        }
        self.delete(at - 1, at)
    }

    /// Delete: the selection, or the character (or paragraph break) after the caret.
    pub fn delete_forward(&mut self) -> Result<(), ModelError> {
        let sel = self.selection;
        if !sel.is_empty() {
            return self.delete(sel.start(), sel.end());
        }
        if sel.head >= self.text.len() {
            return Ok(());
        }
        self.delete(sel.head, sel.head + 1)
    }

    /// Make the whole text `text` (a plain editor's change), keeping the formatting of what
    /// didn't change. The caret goes to the end of what changed.
    pub fn set_plain(&mut self, text: &str) -> Result<(), ModelError> {
        let before = self.text.text();
        self.edit(false, |t, s| {
            t.set_text(text)?;
            let prefix = before.chars().zip(t.text().chars()).take_while(|(a, b)| a == b).count();
            *s = s.clamp(t.len());
            if t.text() != before {
                let suffix = before.chars().rev().zip(t.text().chars().rev()).take_while(|(a, b)| a == b).count();
                let end = t.len().saturating_sub(suffix).max(prefix).min(t.len());
                *s = Selection::caret(end);
            }
            Ok(())
        })?;
        self.typing = None;
        Ok(())
    }

    /// Replace the whole text, as one step (a signature laid out again from its details).
    pub fn replace(&mut self, text: RichText) -> Result<(), ModelError> {
        self.edit(false, |t, s| {
            *t = text;
            *s = s.clamp(t.len());
            Ok(())
        })?;
        self.typing = None;
        Ok(())
    }

    /// Put `sig` in as the message's signature (in place of the one there, else at `at`), or take
    /// it out (`None`), as one step. The caret stays where it was in the text around it.
    pub fn put_signature(&mut self, sig: Option<&RichText>, at: usize) -> Result<(), ModelError> {
        self.edit(false, |t, s| {
            let before = t.len();
            let caret = s.head.min(before);
            // What is at or after this moves with the change; what is before it stays.
            let pivot = t.signature_start().unwrap_or(at);
            match sig {
                Some(sig) => t.put_signature(sig, at)?,
                None => t.remove_signature(),
            }
            let moved = |p: usize| {
                if p < pivot {
                    p
                } else if t.len() >= before {
                    p + (t.len() - before)
                } else {
                    p.saturating_sub(before - t.len()).max(pivot)
                }
            };
            *s = Selection::caret(moved(caret).min(t.len()));
            Ok(())
        })?;
        self.typing = None;
        Ok(())
    }

    /// Put `text` at `at` without moving what is selected (a signature, say), as one step.
    pub fn insert_at(&mut self, at: usize, text: &str, style: &Style) -> Result<(), ModelError> {
        self.edit(false, |t, s| {
            let end = t.insert(at, text, style)?;
            let shift = |p: usize| if p >= at { p + (end - at) } else { p };
            *s = Selection { anchor: shift(s.anchor), head: shift(s.head) };
            Ok(())
        })?;
        self.typing = None;
        Ok(())
    }

    /// Put `with` in place of each of `places` ([`RichText::find`]), as one step: what goes in
    /// looks as the first character of its place did. The selection stays on the text it was on.
    pub fn replace_places(&mut self, places: &[Place], with: &str) -> Result<(), ModelError> {
        let with_chars = super::clean_line(with).chars().count();
        self.edit(false, |t, s| {
            t.replace_places(places, with)?;
            *s = Selection { anchor: moved(s.anchor, places, with_chars), head: moved(s.head, places, with_chars) };
            Ok(())
        })?;
        self.typing = None;
        Ok(())
    }

    /// Turn something on or off for the selection: on unless all of it has it already. With
    /// nothing selected it is the next typed character's. Whether it is on now.
    fn toggle(&mut self, has: fn(&Style) -> bool, set: fn(bool) -> Format) -> Result<bool, ModelError> {
        let sel = self.selection;
        if sel.is_empty() {
            let mut style = self.typing_style();
            let on = !has(&style);
            set(on).apply(&mut style);
            self.pending = Some(style);
            return Ok(on);
        }
        let on = !self.text.all(sel.start(), sel.end(), has)?;
        self.format(set(on))?;
        Ok(on)
    }

    pub fn toggle_bold(&mut self) -> Result<bool, ModelError> {
        self.toggle(|s| s.bold, Format::Bold)
    }

    pub fn toggle_italic(&mut self) -> Result<bool, ModelError> {
        self.toggle(|s| s.italic, Format::Italic)
    }

    pub fn toggle_underline(&mut self) -> Result<bool, ModelError> {
        self.toggle(|s| s.underline, Format::Underline)
    }

    /// Change how the selection looks; with nothing selected, how the next typed character will.
    pub fn format(&mut self, change: Format) -> Result<(), ModelError> {
        let sel = self.selection;
        if sel.is_empty() {
            if let Format::Link(Some(url)) = &change
                && super::safe_link(url).is_none()
            {
                return Err(ModelError::Invalid(
                    "a link goes to an address like https://..., mailto:... or ms-settings:..., never javascript:, vbscript:, data: or file:".into(),
                ));
            }
            let mut style = self.typing_style();
            change.apply(&mut style);
            self.pending = Some(style);
            return Ok(());
        }
        self.edit(false, |t, _| t.format(sel.start(), sel.end(), &change))?;
        self.typing = None;
        Ok(())
    }

    /// Make the selected text a link to `url`, or, with nothing selected, put the address in at
    /// the caret as a link. `None`: remove the link from the selection, or from the whole link
    /// the caret is in.
    pub fn set_link(&mut self, url: Option<&str>) -> Result<(), ModelError> {
        let sel = self.selection;
        match url {
            Some(url) => {
                let url = super::safe_link(url).ok_or_else(|| {
                    ModelError::Invalid(
                        "a link goes to an address like https://..., mailto:... or ms-settings:..., never javascript:, vbscript:, data: or file:"
                            .into(),
                    )
                })?;
                if sel.is_empty() {
                    let mut style = self.typing_style();
                    style.link = Some(url.clone());
                    let shown = url.strip_prefix("mailto:").unwrap_or(&url).to_string();
                    self.edit(false, |t, s| {
                        let end = t.insert(sel.head, &shown, &style)?;
                        *s = Selection::caret(end);
                        Ok(())
                    })?;
                    self.typing = None;
                    Ok(())
                } else {
                    self.format(Format::Link(Some(url)))
                }
            }
            None => {
                let (from, to) =
                    if sel.is_empty() { self.text.link_span(sel.head).unwrap_or((sel.head, sel.head)) } else { (sel.start(), sel.end()) };
                if from == to {
                    self.pending = self.pending.take().map(|mut s| {
                        s.link = None;
                        s
                    });
                    return Ok(());
                }
                self.edit(false, |t, _| t.format(from, to, &Format::Link(None)))?;
                self.typing = None;
                Ok(())
            }
        }
    }

    /// Make the paragraphs the selection touches list items of `kind`, or ordinary paragraphs
    /// when they all are already. What they are now.
    pub fn toggle_list(&mut self, kind: ListKind) -> Result<ListKind, ModelError> {
        let sel = self.selection;
        let now = if self.text.all_lists(sel.start(), sel.end(), kind)? { ListKind::None } else { kind };
        self.edit(false, |t, _| t.set_list(sel.start(), sel.end(), now))?;
        self.typing = None;
        Ok(now)
    }

    /// Move the list items the selection touches a level deeper (`by` 1) or shallower (-1); an
    /// item at the outermost level that goes shallower leaves its list. Whether anything changed.
    pub fn indent(&mut self, by: i8) -> Result<bool, ModelError> {
        let sel = self.selection;
        let before = self.text.clone();
        self.edit(false, |t, _| {
            let touched: Vec<usize> = t.pieces(sel.start(), sel.end())?.iter().map(|(i, _, _)| *i).collect();
            t.indent(sel.start(), sel.end(), by)?;
            if by < 0 {
                for i in touched {
                    let outermost = before.paragraphs.get(i).is_some_and(|b| b.list != ListKind::None && b.level == 0);
                    if let Some(p) = t.paragraphs.get_mut(i).filter(|_| outermost) {
                        p.list = ListKind::None;
                    }
                }
            }
            Ok(())
        })?;
        self.typing = None;
        Ok(self.text != before)
    }

    /// Plain text again: the selection loses its formatting and its paragraphs their bullets
    /// and numbers. With nothing selected, the next typed character is plain.
    pub fn clear_formatting(&mut self) -> Result<(), ModelError> {
        let sel = self.selection;
        self.edit(false, |t, _| {
            t.format(sel.start(), sel.end(), &Format::Clear)?;
            t.set_list(sel.start(), sel.end(), ListKind::None)
        })?;
        if sel.is_empty() {
            self.pending = Some(Style::default());
        }
        self.typing = None;
        Ok(())
    }

    /// Back one step. Whether there was one.
    pub fn undo(&mut self) -> bool {
        let Some(step) = self.undo.pop() else { return false };
        let now = Step { text: std::mem::replace(&mut self.text, step.text), selection: std::mem::replace(&mut self.selection, step.selection) };
        self.redo.push(now);
        self.pending = None;
        self.typing = None;
        true
    }

    /// Forward one step. Whether there was one.
    pub fn redo(&mut self) -> bool {
        let Some(step) = self.redo.pop() else { return false };
        let now = Step { text: std::mem::replace(&mut self.text, step.text), selection: std::mem::replace(&mut self.selection, step.selection) };
        self.undo.push(now);
        self.pending = None;
        self.typing = None;
        true
    }
}
