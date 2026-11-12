// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The import report of a .pst file (#815): for each folder of the file, how many items were
//! found, brought in, already here and left out,
//! each item left out with what it is, its folder and why, the folders not brought, and the
//! damage the reader met. Shown at the end, answered by `file.importPst`, and written as a text
//! file beside the mailbox.
//!
//! What the file holds goes into it (folder names, subjects), so every list is capped and every
//! text cut to size: a hostile file makes a long report, never an unbounded one.

use serde_json::{Value, json};

/// Most items left out listed one by one (the rest are counted).
pub const MAX_LISTED: usize = 2000;
/// Most notes listed (the rest are counted).
pub const MAX_NOTES: usize = 2000;
/// Most folders with a row of their own (the rest are added up in one).
pub const MAX_ROWS: usize = 2000;
/// Most damaged parts of the file listed.
const MAX_DAMAGE: usize = 200;
/// Most characters of a subject or a folder's path in the report.
const MAX_TEXT: usize = 200;

/// One folder of the file, and what came of its items.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Row {
    /// Its path below the top of the file (`Inbox/Projects`; empty for the top itself).
    pub path: String,
    pub found: usize,
    pub added: usize,
    pub already: usize,
    pub left_out: usize,
    /// The folder its mail went into instead, when it is nested deeper than folders here may be.
    pub into: Option<String>,
}

impl Row {
    fn add(&mut self, r: &Row) {
        self.found = self.found.saturating_add(r.found);
        self.added = self.added.saturating_add(r.added);
        self.already = self.already.saturating_add(r.already);
        self.left_out = self.left_out.saturating_add(r.left_out);
    }

    /// Whether every item found is accounted for.
    pub fn adds_up(&self) -> bool {
        self.added.saturating_add(self.already).saturating_add(self.left_out) == self.found
    }
}

/// One thing of the file named in the report: what it is, its folder, and why (or what of it).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub what: String,
    pub folder: String,
    pub why: String,
}

/// The report of one run.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    /// The .pst file's own name (not its path).
    pub file: String,
    /// The folder it went into (made, or picked).
    pub name: String,
    pub dry_run: bool,
    /// "Unicode" or "ANSI".
    pub format: String,
    /// A row for each folder, as many as [`MAX_ROWS`]; the rest added up in `rows_more`.
    pub rows: Vec<Row>,
    pub rows_more: Row,
    /// How many folders `rows_more` adds up.
    pub folders_more: usize,
    /// Items left out, one by one, as many as [`MAX_LISTED`].
    pub left_out: Vec<Entry>,
    pub left_out_more: usize,
    /// What of an item brought in was not (an attachment past a cap, rich text kept as a file,
    /// S/MIME kept whole), as many as [`MAX_NOTES`].
    pub notes: Vec<Entry>,
    pub notes_more: usize,
    /// The file's own folders (the root, searches, views), not brought.
    pub not_brought: Vec<Entry>,
    pub not_brought_more: usize,
    /// What the reader couldn't read of the file's structure.
    pub damage: Vec<String>,
    /// The owner called it off: what was brought in stays, and another run brings the rest.
    pub cancelled: bool,
    /// Contacts and distribution lists (kept as contacts) brought in, of the items added.
    pub contacts_added: usize,
    pub lists_added: usize,
    /// Appointments and tasks found, brought in or not: a file of these alone makes no mail folder.
    pub plans_seen: usize,
    pub events_added: usize,
    pub notes_added: usize,
    pub journal_added: usize,
    pub tasks_added: usize,
    /// What the file holds besides: mail items, and contacts (brought, already here or left out),
    /// as far as the run read them: a file of contacts alone makes no mail folder.
    pub mail_seen: usize,
    pub contacts_seen: usize,
    /// Categories of the file's list added to this computer's.
    pub categories_added: usize,
    /// Where the text of the report was written, when it was.
    pub written: Option<String>,
}

/// `s` without control characters, cut to [`MAX_TEXT`] characters.
pub fn short(s: &str) -> String {
    let mut out: String = s.chars().map(|c| if c.is_control() { ' ' } else { c }).take(MAX_TEXT).collect();
    if s.chars().count() > MAX_TEXT {
        out.push('…');
    }
    out
}

/// A folder's path cut as [`short`] does, but keeping its end, where the folder is named.
pub fn short_path(s: &str) -> String {
    let count = s.chars().count();
    let tail: String = s.chars().skip(count.saturating_sub(MAX_TEXT)).map(|c| if c.is_control() { ' ' } else { c }).collect();
    if count > MAX_TEXT { format!("…{tail}") } else { tail }
}

impl Report {
    pub fn total(&self) -> Row {
        let mut t = self.rows_more.clone();
        for r in &self.rows {
            t.add(r);
        }
        t.path = String::new();
        t
    }

    /// The row of a folder: its own while there is room, else added to `rows_more`.
    pub fn push_row(&mut self, mut row: Row) {
        if self.rows.len() < MAX_ROWS {
            row.path = short_path(&row.path);
            row.into = row.into.map(|i| short_path(&i));
            self.rows.push(row);
        } else {
            self.rows_more.add(&row);
            self.folders_more = self.folders_more.saturating_add(1);
        }
    }

    /// Whether every row adds up (the summed one too).
    pub fn first_short(&self) -> Option<String> {
        if let Some(r) = self.rows.iter().find(|r| !r.adds_up()) {
            return Some(r.path.clone());
        }
        (!self.rows_more.adds_up()).then(|| format!("the {} folders past the report's list", self.folders_more))
    }

    pub fn not_brought(&mut self, what: &str, why: &str) {
        if self.not_brought.len() < MAX_LISTED {
            self.not_brought.push(Entry { what: short(what), folder: String::new(), why: short(why) });
        } else {
            self.not_brought_more = self.not_brought_more.saturating_add(1);
        }
    }

    pub fn leave_out(&mut self, what: &str, folder: &str, why: &str) {
        if self.left_out.len() < MAX_LISTED {
            self.left_out.push(Entry { what: short(what), folder: short(folder), why: short(why) });
        } else {
            self.left_out_more = self.left_out_more.saturating_add(1);
        }
    }

    pub fn note(&mut self, what: &str, folder: &str, note: &str) {
        if self.notes.len() < MAX_NOTES {
            self.notes.push(Entry { what: short(what), folder: short(folder), why: short(note) });
        } else {
            self.notes_more = self.notes_more.saturating_add(1);
        }
    }

    pub fn damaged(&mut self, what: String) {
        if self.damage.len() < MAX_DAMAGE {
            self.damage.push(short(&what));
        }
    }

    /// The answer of `file.importPst`.
    pub fn to_json(&self) -> Value {
        let t = self.total();
        let entries = |list: &[Entry]| list.iter().map(|e| json!({"what": e.what, "folder": e.folder, "why": e.why})).collect::<Vec<_>>();
        json!({
            "file": self.file,
            "name": self.name,
            "dryRun": self.dry_run,
            "format": self.format,
            "found": t.found,
            "added": t.added,
            "messages": t.added.saturating_sub(self.contacts_added).saturating_sub(self.lists_added).saturating_sub(self.events_added).saturating_sub(self.tasks_added).saturating_sub(self.notes_added).saturating_sub(self.journal_added),
            "contacts": self.contacts_added,
            "lists": self.lists_added,
            "events": self.events_added,
            "tasks": self.tasks_added,
            "stickyNotes": self.notes_added,
            "journalEntries": self.journal_added,
            "already": t.already,
            "leftOut": t.left_out,
            "folders": self.rows.iter().map(|r| json!({
                "path": r.path, "found": r.found, "added": r.added, "already": r.already, "leftOut": r.left_out, "into": r.into,
            })).collect::<Vec<_>>(),
            "foldersMore": self.folders_more,
            "leftOutItems": entries(&self.left_out),
            "leftOutMore": self.left_out_more,
            "notes": entries(&self.notes),
            "notesMore": self.notes_more,
            "notBrought": entries(&self.not_brought),
            "notBroughtMore": self.not_brought_more,
            "damage": self.damage,
            "cancelled": self.cancelled,
            "categoriesAdded": self.categories_added,
            "report": self.written,
        })
    }

    /// The report as text, for the file beside the mailbox.
    pub fn to_text(&self, when: &str) -> String {
        let t = self.total();
        let mut out = String::new();
        let mut line = |s: String| {
            out.push_str(&s);
            out.push('\n');
        };
        line(format!("MailCraft: import of the .pst file {} ", short(&self.file)));
        line(format!("When: {when}"));
        if self.name.is_empty() {
            line("Into: this computer".to_string());
        } else {
            line(format!("Into: the folder \"{}\" on this computer", short(&self.name)));
        }
        line(format!("File: {} .pst file", self.format));
        if self.dry_run {
            line("A look only (dry run): nothing was imported; \"brought in\" says what would be.".to_string());
        }
        if self.cancelled {
            line("Cancelled: what was brought in stays; import the file again to bring in the rest.".to_string());
        }
        line(String::new());
        line(format!("Found {} items: {} brought in, {} already here, {} left out.", t.found, t.added, t.already, t.left_out));
        if self.events_added > 0 || self.tasks_added > 0 {
            line(format!("Calendar items brought in: {}; tasks brought in: {}.", self.events_added, self.tasks_added));
        }
        if self.notes_added > 0 || self.journal_added > 0 {
            line(format!("Notes brought in: {}; journal entries brought in: {}.", self.notes_added, self.journal_added));
        }
        if self.contacts_added > 0 || self.lists_added > 0 {
            line(format!(
                "Contacts brought in: {}; distribution lists kept as contacts listing their members: {}.",
                self.contacts_added, self.lists_added
            ));
        }
        if self.categories_added > 0 {
            line(format!("Categories added from the file: {}.", self.categories_added));
        }
        line(String::new());
        line("Folders (found / brought in / already here / left out)".to_string());
        for r in &self.rows {
            let path = if r.path.is_empty() { "(the top of the file)".to_string() } else { r.path.clone() };
            line(format!("  {path}: {} / {} / {} / {}", r.found, r.added, r.already, r.left_out));
            if let Some(into) = &r.into {
                let into = if into.is_empty() { "the folder it was imported into" } else { into.as_str() };
                line(format!("    nested deeper than folders here may be: its mail is in {into}"));
            }
        }
        if self.folders_more > 0 {
            let r = &self.rows_more;
            line(format!("  and {} more folders: {} / {} / {} / {}", self.folders_more, r.found, r.added, r.already, r.left_out));
        }
        let mut list = |title: &str, entries: &[Entry], more: usize| {
            if entries.is_empty() && more == 0 {
                return;
            }
            line(String::new());
            line(title.to_string());
            for e in entries {
                let folder = if e.folder.is_empty() { String::new() } else { format!(" in {}", e.folder) };
                line(format!("  {}{folder}: {}", e.what, e.why));
            }
            if more > 0 {
                line(format!("  and {more} more"));
            }
        };
        list("Not brought (the file's own folders)", &self.not_brought, self.not_brought_more);
        list("Left out", &self.left_out, self.left_out_more);
        list("Brought in, with notes", &self.notes, self.notes_more);
        if !self.damage.is_empty() {
            line(String::new());
            line("Damage in the file".to_string());
            for d in &self.damage {
                line(format!("  {d}"));
            }
        }
        out
    }
}
